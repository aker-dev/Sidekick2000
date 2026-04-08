//! Live streaming transcription with VAD-based chunking.
//!
//! Each audio stream (local mic, remote loopback) gets its own `LiveTranscriber`
//! running on a dedicated worker thread. The transcriber accumulates 16 kHz mono
//! samples, runs Silero VAD to detect speech boundaries, and flushes completed
//! chunks to a WhisperEngine for transcription.
//!
//! Design choice: **two separate WhisperEngine instances** (one per stream) rather
//! than a single shared instance behind Arc<Mutex>. On Apple Silicon (M4 Pro),
//! Metal can schedule work from multiple threads and the GPU has enough capacity
//! for two concurrent small inferences. A shared Mutex would serialize all
//! transcription work and add latency when both speakers talk simultaneously.

use crate::audio::AudioRecorder;
use crate::transcribe::TranscriptSegment;
use crate::whisper_local::WhisperEngine;
use anyhow::Result;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use voice_activity_detector::VoiceActivityDetector;

/// Minimum silence duration (ms) to trigger a chunk flush.
const MIN_SILENCE_MS: u32 = 300;
/// Maximum chunk duration (seconds) before forced flush.
const MAX_CHUNK_SECS: f64 = 10.0;
/// VAD probability threshold: above this = speech.
const VAD_THRESHOLD: f32 = 0.5;
/// VAD chunk size in samples at 16 kHz (512 = 32ms, required by Silero).
const VAD_CHUNK_SIZE: usize = 512;
/// Worker thread polling interval.
const POLL_INTERVAL_MS: u64 = 200;
/// Sample rate used for VAD and Whisper.
const WORK_SAMPLE_RATE: u32 = 16_000;

/// Accumulates audio, detects speech boundaries via VAD, and transcribes
/// completed chunks with Whisper.
pub struct LiveTranscriber {
    engine: WhisperEngine,
    vad: VoiceActivityDetector,
    /// Accumulated 16 kHz mono samples for the current chunk.
    accumulator: Vec<f32>,
    /// Time offset (in seconds) of the first sample in the accumulator,
    /// relative to the shared recording start time.
    chunk_offset_secs: f64,
    /// Total 16 kHz mono samples processed so far (for computing offsets).
    total_samples_processed: u64,
    /// Label for this stream ("local" or "remote").
    stream_label: String,
    /// Number of consecutive silent VAD chunks observed.
    silent_chunks: u32,
    /// Whether we are currently inside a speech region.
    in_speech: bool,
    /// Language code passed to Whisper.
    language: Option<String>,
}

impl LiveTranscriber {
    pub fn new(
        engine: WhisperEngine,
        label: String,
        language: Option<String>,
    ) -> Result<Self> {
        let vad = VoiceActivityDetector::builder()
            .sample_rate(WORK_SAMPLE_RATE)
            .chunk_size(VAD_CHUNK_SIZE)
            .build()
            .map_err(|e| anyhow::anyhow!("Failed to create VAD: {:?}", e))?;

        Ok(Self {
            engine,
            vad,
            accumulator: Vec::with_capacity(WORK_SAMPLE_RATE as usize * MAX_CHUNK_SECS as usize),
            chunk_offset_secs: 0.0,
            total_samples_processed: 0,
            stream_label: label,
            silent_chunks: 0,
            in_speech: false,
            language,
        })
    }

    /// Feed new 16 kHz mono samples. Returns transcribed segments when a
    /// chunk is flushed (silence detected or max duration reached).
    pub fn feed(&mut self, samples: &[f32]) -> Result<Option<Vec<TranscriptSegment>>> {
        if samples.is_empty() {
            return Ok(None);
        }

        // Process samples through VAD in VAD_CHUNK_SIZE increments
        let mut pending_flush = false;

        for chunk in samples.chunks(VAD_CHUNK_SIZE) {
            self.accumulator.extend_from_slice(chunk);
            self.total_samples_processed += chunk.len() as u64;

            // Only run VAD on full-size chunks
            if chunk.len() == VAD_CHUNK_SIZE {
                let prob = self.vad.predict(chunk.to_vec());

                if prob >= VAD_THRESHOLD {
                    self.in_speech = true;
                    self.silent_chunks = 0;
                } else if self.in_speech {
                    self.silent_chunks += 1;
                    let silence_ms =
                        (self.silent_chunks as f64 * VAD_CHUNK_SIZE as f64 / WORK_SAMPLE_RATE as f64 * 1000.0) as u32;
                    if silence_ms >= MIN_SILENCE_MS {
                        pending_flush = true;
                    }
                }
            }

            // Force flush if accumulated audio exceeds max duration
            let accumulated_secs =
                self.accumulator.len() as f64 / WORK_SAMPLE_RATE as f64;
            if accumulated_secs >= MAX_CHUNK_SECS {
                pending_flush = true;
            }
        }

        if pending_flush && !self.accumulator.is_empty() {
            return self.flush_chunk();
        }

        Ok(None)
    }

    /// Flush remaining audio at end of recording.
    pub fn finalize(&mut self) -> Result<Vec<TranscriptSegment>> {
        if self.accumulator.is_empty() {
            return Ok(Vec::new());
        }
        self.flush_chunk()
            .map(|opt| opt.unwrap_or_default())
    }

    /// Transcribe the current accumulator and reset state.
    fn flush_chunk(&mut self) -> Result<Option<Vec<TranscriptSegment>>> {
        let audio = std::mem::take(&mut self.accumulator);
        let offset = self.chunk_offset_secs;

        // Update offset for next chunk
        self.chunk_offset_secs =
            self.total_samples_processed as f64 / WORK_SAMPLE_RATE as f64;
        self.silent_chunks = 0;
        self.in_speech = false;

        // Skip very short chunks (< 0.3s) — likely just noise
        if audio.len() < (WORK_SAMPLE_RATE as f64 * 0.3) as usize {
            log::debug!(
                "[{}] Skipping short chunk: {:.2}s",
                self.stream_label,
                audio.len() as f64 / WORK_SAMPLE_RATE as f64
            );
            return Ok(None);
        }

        log::info!(
            "[{}] Transcribing chunk: {:.2}s at offset {:.2}s",
            self.stream_label,
            audio.len() as f64 / WORK_SAMPLE_RATE as f64,
            offset
        );

        let result = self
            .engine
            .transcribe(&audio, self.language.as_deref(), offset)?;

        if result.segments.is_empty() {
            return Ok(None);
        }

        Ok(Some(result.segments))
    }
}

/// State for a live dual-stream transcription session.
pub struct LiveDualState {
    /// Accumulated segments from the local stream.
    pub local_segments: Arc<Mutex<Vec<TranscriptSegment>>>,
    /// Accumulated segments from the remote stream.
    pub remote_segments: Arc<Mutex<Vec<TranscriptSegment>>>,
    /// Signal to stop worker threads.
    pub stop_signal: Arc<AtomicBool>,
    /// Worker thread handles.
    worker_handles: Vec<std::thread::JoinHandle<()>>,
}

impl LiveDualState {
    /// Spawn worker threads for live transcription of both streams.
    ///
    /// Each worker drains samples from its recorder every ~200ms, feeds them
    /// through VAD + Whisper, and emits Tauri events for live display.
    pub fn start(
        local_recorder: &AudioRecorder,
        remote_recorder: &AudioRecorder,
        model_path: std::path::PathBuf,
        language: Option<String>,
        app: tauri::AppHandle,
    ) -> Result<Self> {
        let stop_signal = Arc::new(AtomicBool::new(false));
        let local_segments: Arc<Mutex<Vec<TranscriptSegment>>> =
            Arc::new(Mutex::new(Vec::new()));
        let remote_segments: Arc<Mutex<Vec<TranscriptSegment>>> =
            Arc::new(Mutex::new(Vec::new()));

        let mut handles = Vec::new();

        // Local stream worker
        {
            let recorder = clone_recorder_refs(local_recorder);
            let segments = local_segments.clone();
            let stop = stop_signal.clone();
            let model = model_path.clone();
            let lang = language.clone();
            let app_handle = app.clone();

            let handle = std::thread::Builder::new()
                .name("live-local".to_string())
                .spawn(move || {
                    if let Err(e) = worker_loop(
                        recorder,
                        &model,
                        "local",
                        lang,
                        segments,
                        stop,
                        app_handle,
                    ) {
                        log::error!("[local] Live transcription worker failed: {}", e);
                    }
                })?;
            handles.push(handle);
        }

        // Remote stream worker (only if recorder has been started)
        {
            let recorder = clone_recorder_refs(remote_recorder);
            let segments = remote_segments.clone();
            let stop = stop_signal.clone();
            let model = model_path;
            let lang = language;
            let app_handle = app;

            let handle = std::thread::Builder::new()
                .name("live-remote".to_string())
                .spawn(move || {
                    if let Err(e) = worker_loop(
                        recorder,
                        &model,
                        "remote",
                        lang,
                        segments,
                        stop,
                        app_handle,
                    ) {
                        log::error!("[remote] Live transcription worker failed: {}", e);
                    }
                })?;
            handles.push(handle);
        }

        Ok(Self {
            local_segments,
            remote_segments,
            stop_signal,
            worker_handles: handles,
        })
    }

    /// Signal workers to stop, wait for them to finish, and return
    /// accumulated segments for both streams.
    pub fn stop(self) -> (Vec<TranscriptSegment>, Vec<TranscriptSegment>) {
        self.stop_signal.store(true, Ordering::SeqCst);

        for handle in self.worker_handles {
            let _ = handle.join();
        }

        let local = self.local_segments.lock().unwrap().clone();
        let remote = self.remote_segments.lock().unwrap().clone();
        (local, remote)
    }
}

/// Lightweight struct holding the Arc refs needed by a worker thread to read
/// from an AudioRecorder without owning it.
struct RecorderRefs {
    samples: Arc<Mutex<std::collections::VecDeque<f32>>>,
    all_samples: Arc<Mutex<Vec<f32>>>,
    sample_rate: Arc<Mutex<u32>>,
    channels: Arc<Mutex<u16>>,
}

fn clone_recorder_refs(recorder: &AudioRecorder) -> RecorderRefs {
    RecorderRefs {
        samples: recorder.samples_ref(),
        all_samples: recorder.all_samples_ref(),
        sample_rate: recorder.sample_rate_ref(),
        channels: recorder.channels_ref(),
    }
}

/// Main worker loop: drain → resample → VAD+Whisper → emit events.
fn worker_loop(
    recorder: RecorderRefs,
    model_path: &std::path::Path,
    label: &str,
    language: Option<String>,
    segments: Arc<Mutex<Vec<TranscriptSegment>>>,
    stop_signal: Arc<AtomicBool>,
    app: tauri::AppHandle,
) -> Result<()> {
    use tauri::Emitter;

    // Each worker gets its own WhisperEngine instance for true GPU parallelism
    let engine = WhisperEngine::new(model_path)?;
    let mut transcriber = LiveTranscriber::new(engine, label.to_string(), language)?;

    log::info!("[{}] Live transcription worker started", label);

    loop {
        std::thread::sleep(std::time::Duration::from_millis(POLL_INTERVAL_MS));

        // Drain samples from the ring buffer
        let drained: Vec<f32>;
        let native_sr: u32;
        let channels: u16;
        {
            let mut buf = recorder.samples.lock().unwrap();
            drained = buf.drain(..).collect();
            // Also save to all_samples for the final WAV
            recorder.all_samples.lock().unwrap().extend_from_slice(&drained);
            native_sr = *recorder.sample_rate.lock().unwrap();
            channels = *recorder.channels.lock().unwrap();
        }

        if !drained.is_empty() {
            // Convert to mono
            let mono = if channels > 1 {
                drained
                    .chunks(channels as usize)
                    .map(|ch| ch.iter().sum::<f32>() / channels as f32)
                    .collect::<Vec<f32>>()
            } else {
                drained
            };

            // Resample to 16 kHz
            let resampled = if native_sr != WORK_SAMPLE_RATE {
                resample_simple(&mono, native_sr, WORK_SAMPLE_RATE)
            } else {
                mono
            };

            // Feed to VAD + Whisper
            match transcriber.feed(&resampled) {
                Ok(Some(new_segments)) => {
                    log::info!(
                        "[{}] Got {} live segments",
                        label,
                        new_segments.len()
                    );

                    // Emit event for frontend
                    let _ = app.emit(
                        "live-segment",
                        serde_json::json!({
                            "speaker": label,
                            "segments": new_segments,
                        }),
                    );

                    // Accumulate for final merge
                    segments.lock().unwrap().extend(new_segments);
                }
                Ok(None) => {}
                Err(e) => {
                    log::error!("[{}] Transcription error: {}", label, e);
                }
            }
        }

        // Check stop signal after processing remaining data
        if stop_signal.load(Ordering::SeqCst) {
            // Drain any final samples that arrived after the stop signal
            let final_drained: Vec<f32>;
            {
                let mut buf = recorder.samples.lock().unwrap();
                final_drained = buf.drain(..).collect();
                recorder.all_samples.lock().unwrap().extend_from_slice(&final_drained);
            }

            if !final_drained.is_empty() {
                let mono = if channels > 1 {
                    final_drained
                        .chunks(channels as usize)
                        .map(|ch| ch.iter().sum::<f32>() / channels as f32)
                        .collect::<Vec<f32>>()
                } else {
                    final_drained
                };
                let resampled = if native_sr != WORK_SAMPLE_RATE {
                    resample_simple(&mono, native_sr, WORK_SAMPLE_RATE)
                } else {
                    mono
                };
                let _ = transcriber.feed(&resampled);
            }

            // Finalize: flush any remaining audio
            match transcriber.finalize() {
                Ok(final_segments) if !final_segments.is_empty() => {
                    log::info!(
                        "[{}] Final flush: {} segments",
                        label,
                        final_segments.len()
                    );
                    let _ = app.emit(
                        "live-segment",
                        serde_json::json!({
                            "speaker": label,
                            "segments": final_segments,
                        }),
                    );
                    segments.lock().unwrap().extend(final_segments);
                }
                Ok(_) => {}
                Err(e) => {
                    log::error!("[{}] Finalize error: {}", label, e);
                }
            }

            log::info!("[{}] Live transcription worker stopped", label);
            break;
        }
    }

    Ok(())
}

/// Simple linear resampling (same as audio.rs / whisper_local.rs).
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
    fn test_vad_silence_detection() {
        // Silence should not trigger speech detection
        let silence = vec![0.0f32; WORK_SAMPLE_RATE as usize]; // 1 second of silence
        let mut vad = VoiceActivityDetector::builder()
            .sample_rate(WORK_SAMPLE_RATE)
            .chunk_size(VAD_CHUNK_SIZE)
            .build()
            .unwrap();

        for chunk in silence.chunks(VAD_CHUNK_SIZE) {
            if chunk.len() == VAD_CHUNK_SIZE {
                let prob = vad.predict(chunk.to_vec());
                assert!(prob < VAD_THRESHOLD, "Silence should have low VAD probability");
            }
        }
    }
}
