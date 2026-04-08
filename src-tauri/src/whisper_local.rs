use crate::transcribe::{TranscriptResult, TranscriptSegment};
use anyhow::{Context, Result};
use std::path::{Path, PathBuf};
use whisper_rs::{FullParams, SamplingStrategy, WhisperContext, WhisperContextParameters};

const MODEL_FILENAME: &str = "ggml-large-v3-turbo-q5_0.bin";
const MODEL_URL: &str =
    "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-large-v3-turbo-q5_0.bin";

/// Returns the directory where Whisper models are stored.
pub fn models_dir() -> PathBuf {
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".sidekick2000")
        .join("models")
}

/// Returns the path to the default Whisper model.
pub fn default_model_path() -> PathBuf {
    models_dir().join(MODEL_FILENAME)
}

/// Download the Whisper model if it doesn't already exist.
/// Emits `model-download-progress` events via the Tauri app handle during download.
pub async fn ensure_model_downloaded(app: Option<&tauri::AppHandle>) -> Result<PathBuf> {
    use tauri::Emitter;

    let model_path = default_model_path();
    if model_path.exists() {
        log::info!("Whisper model already present: {}", model_path.display());
        return Ok(model_path);
    }

    let dir = models_dir();
    std::fs::create_dir_all(&dir)
        .with_context(|| format!("Failed to create models directory: {}", dir.display()))?;

    log::info!("Downloading Whisper model from {}", MODEL_URL);

    let client = reqwest::Client::new();
    let response = client
        .get(MODEL_URL)
        .send()
        .await
        .context("Failed to start model download")?;

    if !response.status().is_success() {
        anyhow::bail!(
            "Model download failed with status: {}",
            response.status()
        );
    }

    let total_size = response.content_length().unwrap_or(0);
    let mut downloaded: u64 = 0;

    // Write to a temp file first, then rename to avoid partial files
    let tmp_path = model_path.with_extension("bin.part");
    let mut file = tokio::fs::File::create(&tmp_path)
        .await
        .with_context(|| format!("Failed to create temp file: {}", tmp_path.display()))?;

    use tokio::io::AsyncWriteExt;
    let mut stream = response.bytes_stream();
    use futures_util::StreamExt;

    while let Some(chunk) = stream.next().await {
        let chunk = chunk.context("Error reading download stream")?;
        file.write_all(&chunk).await?;
        downloaded += chunk.len() as u64;

        if let Some(app) = app {
            let progress = if total_size > 0 {
                downloaded as f64 / total_size as f64
            } else {
                0.0
            };
            let _ = app.emit(
                "model-download-progress",
                serde_json::json!({
                    "downloaded": downloaded,
                    "total": total_size,
                    "progress": progress,
                }),
            );
        }
    }

    file.flush().await?;
    drop(file);

    tokio::fs::rename(&tmp_path, &model_path)
        .await
        .context("Failed to finalize model file")?;

    log::info!(
        "Whisper model downloaded: {} ({:.0} MB)",
        model_path.display(),
        downloaded as f64 / (1024.0 * 1024.0)
    );

    Ok(model_path)
}

/// Local Whisper transcription engine backed by whisper.cpp via whisper-rs.
pub struct WhisperEngine {
    ctx: WhisperContext,
}

impl WhisperEngine {
    /// Create a new engine from a ggml model file.
    pub fn new(model_path: &Path) -> Result<Self> {
        log::info!(
            "Loading Whisper model (Metal acceleration): {}",
            model_path.display()
        );

        let params = WhisperContextParameters::default();
        let ctx = WhisperContext::new_with_params(
            model_path.to_str().context("Invalid model path")?,
            params,
        )
        .map_err(|e| anyhow::anyhow!("Failed to load Whisper model: {}", e))?;

        log::info!("Whisper model loaded successfully");
        Ok(Self { ctx })
    }

    /// Transcribe f32 PCM audio at 16 kHz mono.
    /// `offset_secs` is added to all segment timestamps (useful for chunk-based transcription).
    pub fn transcribe(
        &mut self,
        samples: &[f32],
        language: Option<&str>,
        offset_secs: f64,
    ) -> Result<TranscriptResult> {
        let mut params = FullParams::new(SamplingStrategy::Greedy { best_of: 1 });

        if let Some(lang) = language {
            params.set_language(Some(lang));
        }

        // Disable translation — we want transcription only
        params.set_translate(false);
        // Enable token-level timestamps for better segment boundaries
        params.set_token_timestamps(true);
        // Print progress to stderr (useful for debugging)
        params.set_print_progress(false);
        params.set_print_realtime(false);
        params.set_print_special(false);
        params.set_print_timestamps(false);

        let mut state = self
            .ctx
            .create_state()
            .map_err(|e| anyhow::anyhow!("Failed to create Whisper state: {}", e))?;

        state
            .full(params, samples)
            .map_err(|e| anyhow::anyhow!("Whisper transcription failed: {}", e))?;

        let num_segments = state
            .full_n_segments()
            .map_err(|e| anyhow::anyhow!("Failed to get segment count: {}", e))?;

        let mut segments = Vec::with_capacity(num_segments as usize);
        let mut full_text = String::new();

        for i in 0..num_segments {
            let text = state
                .full_get_segment_text(i)
                .map_err(|e| anyhow::anyhow!("Failed to get segment text: {}", e))?;
            let start = state
                .full_get_segment_t0(i)
                .map_err(|e| anyhow::anyhow!("Failed to get segment t0: {}", e))?;
            let end = state
                .full_get_segment_t1(i)
                .map_err(|e| anyhow::anyhow!("Failed to get segment t1: {}", e))?;

            let trimmed = text.trim().to_string();
            if trimmed.is_empty() {
                continue;
            }

            if !full_text.is_empty() {
                full_text.push(' ');
            }
            full_text.push_str(&trimmed);

            // whisper.cpp timestamps are in centiseconds (10ms units)
            segments.push(TranscriptSegment {
                id: segments.len(),
                start: start as f64 / 100.0 + offset_secs,
                end: end as f64 / 100.0 + offset_secs,
                text: trimmed,
            });
        }

        log::info!(
            "Local Whisper transcription: {} segments, {} chars",
            segments.len(),
            full_text.len()
        );

        Ok(TranscriptResult {
            text: full_text,
            segments,
        })
    }
}

/// Transcribe a WAV file using local Whisper. Reads the WAV, converts to f32
/// 16 kHz mono if needed, and runs WhisperEngine::transcribe.
pub fn transcribe_wav_file(
    engine: &mut WhisperEngine,
    wav_path: &Path,
    language: Option<&str>,
) -> Result<TranscriptResult> {
    log::info!(
        "Transcribing with local Whisper: {}",
        wav_path.display()
    );

    let reader = hound::WavReader::open(wav_path)
        .with_context(|| format!("Failed to open WAV: {}", wav_path.display()))?;

    let spec = reader.spec();
    log::info!(
        "WAV: {} Hz, {} ch, {} bit",
        spec.sample_rate,
        spec.channels,
        spec.bits_per_sample
    );

    // Read samples as f32
    let samples: Vec<f32> = match spec.sample_format {
        hound::SampleFormat::Int => reader
            .into_samples::<i32>()
            .filter_map(|s| s.ok())
            .map(|s| s as f32 / (1 << (spec.bits_per_sample - 1)) as f32)
            .collect(),
        hound::SampleFormat::Float => reader
            .into_samples::<f32>()
            .filter_map(|s| s.ok())
            .collect(),
    };

    // Convert to mono if multi-channel
    let mono = if spec.channels > 1 {
        samples
            .chunks(spec.channels as usize)
            .map(|ch| ch.iter().sum::<f32>() / spec.channels as f32)
            .collect()
    } else {
        samples
    };

    // Resample to 16 kHz if needed (Whisper requirement)
    let resampled = if spec.sample_rate != 16000 {
        resample_simple(&mono, spec.sample_rate, 16000)
    } else {
        mono
    };

    engine.transcribe(&resampled, language, 0.0)
}

/// Simple linear resampling (same algorithm as audio.rs)
fn resample_simple(samples: &[f32], from_sr: u32, to_sr: u32) -> Vec<f32> {
    if from_sr == to_sr {
        return samples.to_vec();
    }
    let ratio = from_sr as f64 / to_sr as f64;
    let new_len = (samples.len() as f64 / ratio) as usize;
    let mut result = Vec::with_capacity(new_len);
    for i in 0..new_len {
        let src_idx = i as f64 * ratio;
        let idx = src_idx as usize;
        let frac = (src_idx - idx as f64) as f32;
        let sample = if idx + 1 < samples.len() {
            samples[idx] * (1.0 - frac) + samples[idx + 1] * frac
        } else if idx < samples.len() {
            samples[idx]
        } else {
            0.0
        };
        result.push(sample);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_models_dir() {
        let dir = models_dir();
        assert!(dir.to_string_lossy().contains(".sidekick2000"));
        assert!(dir.to_string_lossy().ends_with("models"));
    }

    #[test]
    fn test_resample_identity() {
        let samples = vec![1.0, 2.0, 3.0, 4.0];
        let result = resample_simple(&samples, 16000, 16000);
        assert_eq!(result, samples);
    }

    #[test]
    fn test_resample_downsample() {
        let samples: Vec<f32> = (0..32000).map(|i| i as f32 / 32000.0).collect();
        let result = resample_simple(&samples, 32000, 16000);
        assert_eq!(result.len(), 16000);
    }
}
