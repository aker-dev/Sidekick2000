use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum TranscriptionMode {
    Groq,
    LocalWhisper,
}

impl Default for TranscriptionMode {
    fn default() -> Self {
        Self::LocalWhisper
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Context {
    pub id: String,
    pub label: String,
    pub content: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Speaker {
    pub name: String,
    pub organization: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Settings {
    #[serde(default)]
    pub groq_api_key: String,
    #[serde(default)]
    pub anthropic_api_key: String,
    #[serde(default)]
    pub working_folder: String,
    #[serde(default)]
    pub github_repo: String,
    #[serde(default = "default_meetings_subfolder")]
    pub meetings_subfolder: String,
    #[serde(default = "default_language")]
    pub default_language: String,
    #[serde(default)]
    pub default_speakers: Vec<Speaker>,
    #[serde(default)]
    pub contexts: Vec<Context>,
    /// Device name for the local microphone (the user's own mic).
    #[serde(default)]
    pub default_input_device: String,
    /// Display name for the local speaker (shown in transcript and summary).
    #[serde(default = "default_local_speaker_name")]
    pub local_speaker_name: String,
    /// Device name for the remote audio source (system audio / virtual cable).
    #[serde(default)]
    pub remote_device: String,
    /// Display name for the remote speaker (shown in transcript and summary).
    #[serde(default = "default_remote_speaker_name")]
    pub remote_speaker_name: String,
    #[serde(default)]
    pub together_ai_api_key: String,
    /// "claude" or "together_ai"
    #[serde(default = "default_summarization_provider")]
    pub summarization_provider: String,
    #[serde(default = "default_together_ai_model")]
    pub together_ai_model: String,
    /// Transcription engine: LocalWhisper (default, offline) or Groq (cloud).
    #[serde(default)]
    pub transcription_mode: TranscriptionMode,
    #[serde(default = "default_true")]
    pub enable_summary: bool,
    #[serde(default = "default_true")]
    pub enable_git_commit: bool,
    #[serde(default = "default_true")]
    pub enable_github_issues: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            groq_api_key: String::new(),
            anthropic_api_key: String::new(),
            working_folder: String::new(),
            github_repo: String::new(),
            meetings_subfolder: default_meetings_subfolder(),
            default_language: default_language(),
            default_speakers: Vec::new(),
            contexts: Vec::new(),
            default_input_device: String::new(),
            local_speaker_name: default_local_speaker_name(),
            remote_device: String::new(),
            remote_speaker_name: default_remote_speaker_name(),
            together_ai_api_key: String::new(),
            summarization_provider: default_summarization_provider(),
            together_ai_model: default_together_ai_model(),
            transcription_mode: TranscriptionMode::default(),
            enable_summary: true,
            enable_git_commit: true,
            enable_github_issues: true,
        }
    }
}

fn default_true() -> bool {
    true
}

fn default_local_speaker_name() -> String {
    "Local".to_string()
}

fn default_remote_speaker_name() -> String {
    "Remote".to_string()
}

fn default_meetings_subfolder() -> String {
    "Meetings".to_string()
}

fn default_language() -> String {
    "fr".to_string()
}

fn default_summarization_provider() -> String {
    "claude".to_string()
}

fn default_together_ai_model() -> String {
    "meta-llama/Llama-3.3-70B-Instruct-Turbo".to_string()
}

pub fn settings_path() -> PathBuf {
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".sidekick2000")
        .join("settings.json")
}

pub fn load() -> Settings {
    let path = settings_path();
    if !path.exists() {
        return Settings::default();
    }
    match std::fs::read_to_string(&path) {
        Ok(content) => serde_json::from_str(&content).unwrap_or_default(),
        Err(_) => Settings::default(),
    }
}

pub fn save(settings: &Settings) -> Result<()> {
    let path = settings_path();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let content = serde_json::to_string_pretty(settings)?;
    std::fs::write(&path, content)?;
    Ok(())
}
