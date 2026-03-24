use crate::diarize;
use crate::export;
use crate::github;
use crate::merge;
use crate::summarize;
use crate::transcribe;
use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use tauri::{AppHandle, Emitter};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Speaker {
    pub name: String,
    pub organization: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PipelineConfig {
    pub context: String,
    pub context_content: String,
    pub speakers: Vec<Speaker>,
    /// ISO 639-1 code for Groq transcription (e.g. "fr", "en")
    pub language_code: String,
    /// Full language name for Claude summarization (e.g. "French", "English")
    pub language_name: String,
    /// Optional GitHub repo (e.g. "owner/repo") to create issues for action items
    pub github_repo: String,
    pub output_dir: String,
    /// Git repo root folder for committing notes (empty = no commit)
    pub working_folder: String,
    pub meeting_name: String,
    pub ogg_path: String,
    pub wav_path: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PipelineResult {
    pub notes_path: String,
    pub created_issues: Vec<github::CreatedIssue>,
}

#[derive(Clone, Serialize)]
pub struct PipelineProgress {
    pub step: String,
    pub progress: f64,
}

fn emit_progress(app: &AppHandle, step: &str, progress: f64) {
    let _ = app.emit(
        "pipeline-progress",
        PipelineProgress {
            step: step.to_string(),
            progress,
        },
    );
}

/// Commit files to the working_folder git repo
fn git_commit_notes(working_folder: &str, notes_rel: &str, transcript_rel: &str, message: &str) {
    let add = std::process::Command::new("git")
        .current_dir(working_folder)
        .args(["add", notes_rel, transcript_rel])
        .output();

    if let Ok(o) = add {
        if o.status.success() {
            let _ = std::process::Command::new("git")
                .current_dir(working_folder)
                .args(["commit", "-m", message])
                .output();
        } else {
            log::warn!(
                "git add failed: {}",
                String::from_utf8_lossy(&o.stderr)
            );
        }
    }
}

/// Run the full processing pipeline
pub async fn run(
    config: PipelineConfig,
    groq_key: String,
    anthropic_key: String,
    together_key: String,
    summarization_provider: String,
    together_model: String,
    enable_summary: bool,
    enable_git_commit: bool,
    enable_github_issues: bool,
    app: AppHandle,
) -> Result<PipelineResult> {
    let ogg_path = PathBuf::from(&config.ogg_path);
    let wav_path = PathBuf::from(&config.wav_path);

    // Step 1: Transcribe (async API call)
    emit_progress(&app, "transcribing", 0.0);
    let language: Option<String> = if config.language_code.is_empty() {
        None
    } else {
        Some(config.language_code.clone())
    };

    // Run transcription and diarization concurrently
    let groq_key_clone = groq_key.clone();
    let ogg_path_clone = ogg_path.clone();
    let wav_path_clone = wav_path.clone();
    let max_speakers = config.speakers.len().max(2);

    let transcript_handle = tokio::spawn(async move {
        transcribe::transcribe_with_groq(
            &ogg_path_clone,
            language.as_deref(),
            &groq_key_clone,
        )
        .await
    });

    emit_progress(&app, "diarizing", 0.15);
    let diarize_handle = tokio::task::spawn_blocking(move || {
        diarize::diarize(&wav_path_clone, 1, max_speakers)
    });

    let transcript = transcript_handle.await??;
    emit_progress(&app, "diarizing", 0.30);

    let diarization = diarize_handle.await??;
    emit_progress(&app, "merging", 0.50);

    // Step 3: Merge
    let merged = merge::merge(&transcript.segments, &diarization);

    // Step 4: Generate transcript markdown
    let transcript_md = export::export_transcript_markdown(&merged);

    // Step 5: Summarize
    let notes = if enable_summary {
        emit_progress(&app, "summarizing", 0.60);

        let speaker_pairs: Vec<(String, String)> = config
            .speakers
            .iter()
            .map(|s| (s.name.clone(), s.organization.clone()))
            .collect();

        if summarization_provider == "together_ai" {
            summarize::summarize_with_together(
                &transcript_md,
                &config.context_content,
                &speaker_pairs,
                &config.language_name,
                &together_key,
                &together_model,
            )
            .await?
        } else {
            summarize::summarize_with_claude(
                &transcript_md,
                &config.context_content,
                &speaker_pairs,
                &config.language_name,
                &anthropic_key,
            )
            .await?
        }
    } else {
        transcript_md.clone()
    };

    // Step 6: Export
    emit_progress(&app, "exporting", 0.90);

    let output_dir = PathBuf::from(&config.output_dir);
    std::fs::create_dir_all(&output_dir)?;

    // File naming: YYYY-MM-DD_HHmm_Context_MeetingName.md
    let now = chrono::Local::now();
    let date = now.format("%Y-%m-%d").to_string();
    let time = now.format("%H%M").to_string();
    let context_sanitized = export::sanitize_filename(&config.context);
    let meeting_name_sanitized = export::sanitize_filename(&config.meeting_name);
    let base_name = match (context_sanitized.is_empty(), meeting_name_sanitized.is_empty()) {
        (true, true) => format!("{}_{}_meeting", date, time),
        (false, true) => format!("{}_{}_{}", date, time, context_sanitized),
        (true, false) => format!("{}_{}_{}", date, time, meeting_name_sanitized),
        (false, false) => format!("{}_{}_{}_{}", date, time, context_sanitized, meeting_name_sanitized),
    };

    let filename = format!("{}.md", base_name);
    let transcript_filename = format!("{}_transcript.md", base_name);

    let output_path = output_dir.join(&filename);
    let transcript_path = output_dir.join(&transcript_filename);

    std::fs::write(&output_path, &notes)?;
    log::info!("Meeting notes saved to: {}", output_path.display());

    std::fs::write(&transcript_path, &transcript_md)?;

    // Step 7: Git commit (if enabled and working_folder is set)
    if enable_git_commit && !config.working_folder.is_empty() {
        emit_progress(&app, "committing", 0.93);
        let working_folder = &config.working_folder;

        // Compute paths relative to the git root
        let notes_rel = output_path
            .strip_prefix(working_folder)
            .map(|p| p.to_string_lossy().to_string())
            .unwrap_or_else(|_| output_path.to_string_lossy().to_string());
        let transcript_rel = transcript_path
            .strip_prefix(working_folder)
            .map(|p| p.to_string_lossy().to_string())
            .unwrap_or_else(|_| transcript_path.to_string_lossy().to_string());

        let commit_label = if !meeting_name_sanitized.is_empty() {
            meeting_name_sanitized.clone()
        } else if !context_sanitized.is_empty() {
            context_sanitized.clone()
        } else {
            "general".to_string()
        };
        let commit_msg = format!("meeting: {} {}", commit_label, date);

        git_commit_notes(working_folder, &notes_rel, &transcript_rel, &commit_msg);
        log::info!("Git commit done: {}", commit_msg);
    }

    // Step 8: Create GitHub issues from action items (optional)
    let mut created_issues = Vec::new();
    if enable_github_issues && !config.github_repo.is_empty() {
        emit_progress(&app, "creating_issues", 0.95);

        let action_items = github::parse_action_items(&notes);
        log::info!(
            "Found {} action items to create as issues",
            action_items.len()
        );

        if !action_items.is_empty() {
            let issue_label = if config.meeting_name.is_empty() {
                &config.context
            } else {
                &config.meeting_name
            };
            created_issues = github::create_issues(
                &config.github_repo,
                &action_items,
                issue_label,
                &date,
                &output_path.to_string_lossy(),
            );
        }
    }

    emit_progress(&app, "done", 1.0);

    Ok(PipelineResult {
        notes_path: output_path.to_string_lossy().to_string(),
        created_issues,
    })
}
