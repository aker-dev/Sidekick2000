mod audio;
mod diarize;
mod export;
mod github;
mod merge;
mod pipeline;
mod settings;
mod summarize;
mod transcribe;

use audio::{list_input_devices, AudioRecorder};
use pipeline::PipelineConfig;
use std::path::PathBuf;
use std::sync::Mutex;

/// Application state shared across commands
struct AppState {
    recorder: AudioRecorder,
    temp_dir: PathBuf,
}

#[tauri::command]
fn list_input_devices_cmd() -> Vec<String> {
    list_input_devices()
}

#[tauri::command]
async fn start_monitoring(
    state: tauri::State<'_, Mutex<AppState>>,
    device_name: Option<String>,
) -> Result<(), String> {
    let state = state.lock().map_err(|e| e.to_string())?;
    state
        .recorder
        .start_monitor(device_name)
        .map_err(|e| format!("Failed to start monitor: {}", e))
}

#[tauri::command]
fn stop_monitoring(state: tauri::State<'_, Mutex<AppState>>) -> Result<(), String> {
    let state = state.lock().map_err(|e| e.to_string())?;
    state.recorder.stop_monitor();
    Ok(())
}

#[tauri::command]
async fn start_recording(
    state: tauri::State<'_, Mutex<AppState>>,
    _app: tauri::AppHandle,
    device_name: Option<String>,
) -> Result<(), String> {
    let state = state.lock().map_err(|e| e.to_string())?;
    state
        .recorder
        .start(device_name)
        .map_err(|e| format!("Failed to start recording: {}", e))
}

#[tauri::command]
async fn stop_recording(
    state: tauri::State<'_, Mutex<AppState>>,
) -> Result<(String, String), String> {
    let state = state.lock().map_err(|e| e.to_string())?;
    let (ogg_path, wav_path) = state
        .recorder
        .stop(&state.temp_dir)
        .map_err(|e| format!("Failed to stop recording: {}", e))?;

    Ok((
        ogg_path.to_string_lossy().to_string(),
        wav_path.to_string_lossy().to_string(),
    ))
}

#[tauri::command]
async fn get_audio_level(state: tauri::State<'_, Mutex<AppState>>) -> Result<f32, String> {
    let state = state.lock().map_err(|e| e.to_string())?;
    Ok(state.recorder.current_level())
}

#[tauri::command]
async fn get_elapsed(state: tauri::State<'_, Mutex<AppState>>) -> Result<f64, String> {
    let state = state.lock().map_err(|e| e.to_string())?;
    Ok(state.recorder.elapsed_secs())
}

#[tauri::command]
async fn is_recording(state: tauri::State<'_, Mutex<AppState>>) -> Result<bool, String> {
    let state = state.lock().map_err(|e| e.to_string())?;
    Ok(state.recorder.is_recording())
}

#[tauri::command]
async fn run_pipeline(
    config: PipelineConfig,
    app: tauri::AppHandle,
) -> Result<pipeline::PipelineResult, String> {
    // Load settings to get API keys (fallback to env vars)
    let s = settings::load();

    let groq_key = if !s.groq_api_key.is_empty() {
        s.groq_api_key.clone()
    } else {
        std::env::var("GROQ_API_KEY")
            .map_err(|_| "GROQ_API_KEY not set. Configure it in Settings or .env file.")?
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

    // Validate that the required key for the selected provider is present
    if summarization_provider == "together_ai" && together_key.is_empty() {
        return Err("Together.ai API key not set. Configure it in Settings.".to_string());
    } else if summarization_provider != "together_ai" && anthropic_key.is_empty() {
        return Err("ANTHROPIC_API_KEY not set. Configure it in Settings or .env file.".to_string());
    }

    pipeline::run(config, groq_key, anthropic_key, together_key, summarization_provider, together_model, app)
        .await
        .map_err(|e| format!("Pipeline failed: {}", e))
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
        .manage(Mutex::new(AppState {
            recorder: AudioRecorder::new(),
            temp_dir,
        }))
        .invoke_handler(tauri::generate_handler![
            list_input_devices_cmd,
            start_monitoring,
            stop_monitoring,
            start_recording,
            stop_recording,
            get_audio_level,
            get_elapsed,
            is_recording,
            run_pipeline,
            get_default_output_dir,
            open_file,
            get_settings,
            save_settings,
            save_input_device,
            prepare_dropped_audio,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
