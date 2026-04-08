mod audio;
mod export;
mod github;
mod live_transcribe;
mod merge;
mod pipeline;
mod settings;
mod summarize;
mod transcribe;
mod whisper_local;

use audio::{list_input_devices, AudioRecorder, CategorizedDevices};
use pipeline::PipelineConfig;
use std::path::PathBuf;
use std::sync::Mutex;
use transcribe::TranscriptSegment;

/// Application state shared across commands
struct AppState {
    /// Recorder for the local microphone.
    local_recorder: AudioRecorder,
    /// Recorder for the remote audio source (system audio / virtual cable).
    /// Shares `start_time` with `local_recorder` so both streams have a
    /// common t=0 origin.
    remote_recorder: AudioRecorder,
    temp_dir: PathBuf,
    /// Active live transcription session (None when not recording or in Groq mode).
    live_state: Option<live_transcribe::LiveDualState>,
    /// Pre-computed segments from the last live transcription session.
    /// Consumed by the next pipeline run, then cleared.
    live_local_segments: Option<Vec<TranscriptSegment>>,
    live_remote_segments: Option<Vec<TranscriptSegment>>,
}

#[tauri::command]
fn list_input_devices_cmd() -> Vec<String> {
    list_input_devices()
}

/// Returns input devices split into microphones and loopback (BlackHole) categories.
#[tauri::command]
fn list_audio_devices() -> CategorizedDevices {
    audio::list_audio_devices_categorized()
}

/// Start level-monitoring streams on both devices (no sample accumulation).
/// `remote_device` may be None if no remote source is configured.
#[tauri::command]
async fn start_monitoring(
    state: tauri::State<'_, Mutex<AppState>>,
    local_device: Option<String>,
    remote_device: Option<String>,
) -> Result<(), String> {
    let state = state.lock().map_err(|e| e.to_string())?;
    state
        .local_recorder
        .start_monitor(local_device)
        .map_err(|e| format!("Failed to start local monitor: {}", e))?;
    if let Some(dev) = remote_device {
        if !dev.is_empty() {
            let _ = state.remote_recorder.start_monitor(Some(dev));
        }
    }
    Ok(())
}

#[tauri::command]
fn stop_monitoring(state: tauri::State<'_, Mutex<AppState>>) -> Result<(), String> {
    let state = state.lock().map_err(|e| e.to_string())?;
    state.local_recorder.stop_monitor();
    state.remote_recorder.stop_monitor();
    Ok(())
}

/// Start recording on both devices simultaneously.
/// In LocalWhisper mode, also spawns live transcription worker threads.
#[tauri::command]
async fn start_recording(
    state: tauri::State<'_, Mutex<AppState>>,
    local_device: Option<String>,
    remote_device: Option<String>,
    app: tauri::AppHandle,
) -> Result<(), String> {
    let s = settings::load();

    let mut state = state.lock().map_err(|e| e.to_string())?;

    // Clear any pre-computed segments from a previous session
    state.live_local_segments = None;
    state.live_remote_segments = None;

    state
        .local_recorder
        .start(local_device)
        .map_err(|e| format!("Failed to start local recording: {}", e))?;

    let has_remote = if let Some(dev) = remote_device {
        if !dev.is_empty() {
            state
                .remote_recorder
                .start(Some(dev))
                .map_err(|e| format!("Failed to start remote recording: {}", e))?;
            true
        } else {
            false
        }
    } else {
        false
    };

    // Launch live transcription workers in LocalWhisper mode
    if s.transcription_mode == settings::TranscriptionMode::LocalWhisper {
        let model_path = whisper_local::default_model_path();
        if !model_path.exists() {
            // Model not downloaded yet — skip live transcription, will batch-transcribe on stop
            log::warn!(
                "Whisper model not found at {}. Live transcription disabled for this session.",
                model_path.display()
            );
        } else {
            let language = if s.default_language.is_empty() {
                None
            } else {
                Some(s.default_language.clone())
            };

            match live_transcribe::LiveDualState::start(
                &state.local_recorder,
                &state.remote_recorder,
                model_path,
                language,
                app,
            ) {
                Ok(live_state) => {
                    state.live_state = Some(live_state);
                    log::info!(
                        "Live transcription started (local{})",
                        if has_remote { " + remote" } else { "" }
                    );
                }
                Err(e) => {
                    log::error!("Failed to start live transcription: {}. Will batch-transcribe on stop.", e);
                }
            }
        }
    }

    Ok(())
}

/// Stop both recorders and return (local_ogg, local_wav, remote_ogg, remote_wav).
/// Remote paths are empty strings if no remote stream was recorded.
/// If live transcription was active, finalizes it and stores segments for the pipeline.
#[tauri::command]
async fn stop_recording(
    state: tauri::State<'_, Mutex<AppState>>,
) -> Result<(String, String, String, String), String> {
    let mut state = state.lock().map_err(|e| e.to_string())?;

    // Finalize live transcription before stopping recorders (so workers can
    // drain the last samples from the ring buffer).
    if let Some(live_state) = state.live_state.take() {
        log::info!("Finalizing live transcription...");
        let (local_segs, remote_segs) = live_state.stop();
        log::info!(
            "Live transcription finalized: {} local segments, {} remote segments",
            local_segs.len(),
            remote_segs.len()
        );
        state.live_local_segments = Some(local_segs);
        state.live_remote_segments = Some(remote_segs);
    }

    let (local_ogg, local_wav) = state
        .local_recorder
        .stop(&state.temp_dir, "local")
        .map_err(|e| format!("Failed to stop local recording: {}", e))?;

    let (remote_ogg, remote_wav) = if state.remote_recorder.has_samples() {
        state
            .remote_recorder
            .stop(&state.temp_dir, "remote")
            .map_err(|e| format!("Failed to stop remote recording: {}", e))
            .map(|(o, w)| (o.to_string_lossy().to_string(), w.to_string_lossy().to_string()))
            .unwrap_or_default()
    } else {
        (String::new(), String::new())
    };

    Ok((
        local_ogg.to_string_lossy().to_string(),
        local_wav.to_string_lossy().to_string(),
        remote_ogg,
        remote_wav,
    ))
}

/// Returns current RMS levels for (local, remote) streams.
#[tauri::command]
async fn get_audio_levels(state: tauri::State<'_, Mutex<AppState>>) -> Result<(f32, f32), String> {
    let state = state.lock().map_err(|e| e.to_string())?;
    Ok((
        state.local_recorder.current_level(),
        state.remote_recorder.current_level(),
    ))
}

#[tauri::command]
async fn get_elapsed(state: tauri::State<'_, Mutex<AppState>>) -> Result<f64, String> {
    let state = state.lock().map_err(|e| e.to_string())?;
    Ok(state.local_recorder.elapsed_secs())
}

#[tauri::command]
async fn is_recording(state: tauri::State<'_, Mutex<AppState>>) -> Result<bool, String> {
    let state = state.lock().map_err(|e| e.to_string())?;
    Ok(state.local_recorder.is_recording())
}

#[tauri::command]
async fn run_pipeline(
    config: PipelineConfig,
    state: tauri::State<'_, Mutex<AppState>>,
    app: tauri::AppHandle,
) -> Result<pipeline::PipelineResult, String> {
    // Load settings to get API keys (fallback to env vars)
    let s = settings::load();

    let transcription_mode = s.transcription_mode.clone();

    // Take pre-computed live segments if available (consumed once)
    let (live_local, live_remote) = {
        let mut st = state.lock().map_err(|e| e.to_string())?;
        (st.live_local_segments.take(), st.live_remote_segments.take())
    };

    // Groq key is only required in Groq mode without pre-computed segments
    let groq_key = if transcription_mode == settings::TranscriptionMode::Groq && live_local.is_none() {
        if !s.groq_api_key.is_empty() {
            s.groq_api_key.clone()
        } else {
            std::env::var("GROQ_API_KEY")
                .map_err(|_| "GROQ_API_KEY not set. Configure it in Settings or .env file.")?
        }
    } else {
        s.groq_api_key.clone()
    };

    let anthropic_key = if !s.anthropic_api_key.is_empty() {
        s.anthropic_api_key.clone()
    } else {
        std::env::var("ANTHROPIC_API_KEY").unwrap_or_default()
    };

    let together_key = if !s.together_ai_api_key.is_empty() {
        s.together_ai_api_key.clone()
    } else {
        std::env::var("TOGETHER_API_KEY").unwrap_or_default()
    };

    let summarization_provider = s.summarization_provider.clone();
    let together_model = s.together_ai_model.clone();
    let enable_summary = s.enable_summary;
    let enable_git_commit = s.enable_git_commit;
    let enable_github_issues = s.enable_github_issues;

    // Validate that the required key for the selected provider is present (only when summary is enabled)
    if enable_summary {
        if summarization_provider == "together_ai" && together_key.is_empty() {
            return Err("Together.ai API key not set. Configure it in Settings.".to_string());
        } else if summarization_provider != "together_ai" && anthropic_key.is_empty() {
            return Err("ANTHROPIC_API_KEY not set. Configure it in Settings or .env file.".to_string());
        }
    }

    pipeline::run(
        config,
        transcription_mode,
        live_local,
        live_remote,
        groq_key,
        anthropic_key,
        together_key,
        summarization_provider,
        together_model,
        enable_summary,
        enable_git_commit,
        enable_github_issues,
        app,
    )
    .await
    .map_err(|e| format!("Pipeline failed: {}", e))
}

/// Returns the current Whisper model download status.
#[tauri::command]
fn get_model_download_status() -> serde_json::Value {
    let model_path = whisper_local::default_model_path();
    let exists = model_path.exists();
    let size_bytes = if exists {
        std::fs::metadata(&model_path).map(|m| m.len()).unwrap_or(0)
    } else {
        0
    };
    serde_json::json!({
        "downloaded": exists,
        "path": model_path.to_string_lossy(),
        "size_bytes": size_bytes,
    })
}

/// Trigger model download. Returns when the download completes.
/// Emits `model-download-progress` events during download.
#[tauri::command]
async fn download_whisper_model(app: tauri::AppHandle) -> Result<String, String> {
    let path = whisper_local::ensure_model_downloaded(Some(&app))
        .await
        .map_err(|e| format!("Model download failed: {}", e))?;
    Ok(path.to_string_lossy().to_string())
}

#[tauri::command]
fn get_default_output_dir() -> String {
    let s = settings::load();
    if !s.working_folder.is_empty() {
        let subfolder = if s.meetings_subfolder.is_empty() {
            "Meetings".to_string()
        } else {
            s.meetings_subfolder.clone()
        };
        return PathBuf::from(&s.working_folder)
            .join(&subfolder)
            .to_string_lossy()
            .to_string();
    }
    let home = dirs::document_dir().unwrap_or_else(|| PathBuf::from("."));
    home.join("Sidekick2000").to_string_lossy().to_string()
}

#[tauri::command]
async fn open_file(path: String) -> Result<(), String> {
    open::that(&path).map_err(|e| format!("Failed to open file: {}", e))
}

#[tauri::command]
fn get_settings() -> Result<settings::Settings, String> {
    Ok(settings::load())
}

#[tauri::command]
fn save_settings(s: settings::Settings) -> Result<(), String> {
    settings::save(&s).map_err(|e| format!("Failed to save settings: {}", e))
}

#[tauri::command]
fn save_input_device(name: String) -> Result<(), String> {
    let mut s = settings::load();
    s.default_input_device = name;
    settings::save(&s).map_err(|e| format!("Failed to save input device: {}", e))
}

/// Decode a base64-encoded image pasted from the clipboard, save it to the
/// temp directory, and return the absolute path. `extension` should be "png"
/// or "jpeg". `timecode_secs` is used to derive a unique filename.
#[tauri::command]
async fn save_pasted_image(
    data_base64: String,
    extension: String,
    timecode_secs: f64,
    state: tauri::State<'_, Mutex<AppState>>,
) -> Result<String, String> {
    use base64::{engine::general_purpose, Engine as _};
    let bytes = general_purpose::STANDARD
        .decode(data_base64.trim())
        .map_err(|e| format!("Failed to decode image: {}", e))?;

    let temp_dir = {
        let s = state.lock().map_err(|e| e.to_string())?;
        s.temp_dir.clone()
    };

    let ext = if extension.is_empty() { "png" } else { &extension };
    let filename = format!("screenshot_{:06.0}.{}", timecode_secs, ext);
    let path = temp_dir.join(&filename);
    std::fs::write(&path, &bytes)
        .map_err(|e| format!("Failed to save image: {}", e))?;

    Ok(path.to_string_lossy().to_string())
}

/// Decode a dropped audio file (any format supported by symphonia) and convert
/// it to OGG/Opus + WAV at 16 kHz mono. Returns (ogg_path, wav_path).
#[tauri::command]
async fn prepare_dropped_audio(
    path: String,
    state: tauri::State<'_, Mutex<AppState>>,
) -> Result<(String, String), String> {
    let p = std::path::Path::new(&path);
    if !p.exists() {
        return Err(format!("File not found: {}", path));
    }
    let temp_dir = {
        let s = state.lock().map_err(|e| e.to_string())?;
        s.temp_dir.clone()
    };
    let input = p.to_path_buf();
    let (ogg_path, wav_path) = tokio::task::spawn_blocking(move || {
        audio::prepare_audio_file(&input, &temp_dir)
    })
    .await
    .map_err(|e| e.to_string())?
    .map_err(|e| format!("Failed to prepare audio: {}", e))?;

    Ok((
        ogg_path.to_string_lossy().to_string(),
        wav_path.to_string_lossy().to_string(),
    ))
}

pub fn run() {
    // Load .env file as fallback
    let _ = dotenvy::dotenv();
    env_logger::init();

    let temp_dir = std::env::temp_dir().join("sidekick2000");
    let _ = std::fs::create_dir_all(&temp_dir);

    tauri::Builder::default()
        .plugin(tauri_plugin_shell::init())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_fs::init())
        .manage(Mutex::new({
            let local_recorder = AudioRecorder::new();
            let shared_start = local_recorder.shared_start_time();
            let remote_recorder = AudioRecorder::new_with_shared_start_time(shared_start);
            AppState {
                local_recorder,
                remote_recorder,
                temp_dir,
                live_state: None,
                live_local_segments: None,
                live_remote_segments: None,
            }
        }))
        .invoke_handler(tauri::generate_handler![
            list_input_devices_cmd,
            list_audio_devices,
            start_monitoring,
            stop_monitoring,
            start_recording,
            stop_recording,
            get_audio_levels,
            get_elapsed,
            is_recording,
            run_pipeline,
            get_model_download_status,
            download_whisper_model,
            get_default_output_dir,
            open_file,
            get_settings,
            save_settings,
            save_input_device,
            save_pasted_image,
            prepare_dropped_audio,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
