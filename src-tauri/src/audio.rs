use anyhow::{Context, Result};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::SampleFormat;
use hound::{WavSpec, WavWriter};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;

/// Returns the names of all available input devices.
pub fn list_input_devices() -> Vec<String> {
    let host = cpal::default_host();
    host.input_devices()
        .map(|devices| {
            devices
                .filter_map(|d| d.name().ok())
                .collect()
        })
        .unwrap_or_default()
}

/// Manages audio recording from the default input device.
///
/// Because `cpal::Stream` is not `Send` on macOS, we store the stream
/// behind an `Arc<Mutex<Option<...>>>` and keep it on the creating thread.
/// The stream's data callback pushes samples into a shared buffer.
pub struct AudioRecorder {
    is_recording: Arc<AtomicBool>,
    samples: Arc<Mutex<Vec<f32>>>,
    sample_rate: Arc<Mutex<u32>>,
    channels: Arc<Mutex<u16>>,
    start_time: Arc<Mutex<Option<Instant>>>,
}

impl AudioRecorder {
    pub fn new() -> Self {
        Self {
            is_recording: Arc::new(AtomicBool::new(false)),
            samples: Arc::new(Mutex::new(Vec::new())),
            sample_rate: Arc::new(Mutex::new(44100)),
            channels: Arc::new(Mutex::new(1)),
            start_time: Arc::new(Mutex::new(None)),
        }
    }

    /// Start recording from the specified input device, or the default if `device_name` is None.
    /// The stream is kept alive by a dedicated non-Send thread
    /// spawned via `dispatch` on macOS.
    pub fn start(&self, device_name: Option<String>) -> Result<()> {
        if self.is_recording.load(Ordering::SeqCst) {
            anyhow::bail!("Already recording");
        }

        // Clear previous samples
        self.samples.lock().unwrap().clear();

        let host = cpal::default_host();
        let device = if let Some(name) = device_name {
            host.input_devices()
                .context("Failed to enumerate input devices")?
                .find(|d| d.name().map(|n| n == name).unwrap_or(false))
                .with_context(|| format!("Input device not found: {}", name))?
        } else {
            host.default_input_device()
                .context("No input device available")?
        };

        log::info!("Recording from: {}", device.name().unwrap_or_default());

        let config = device.default_input_config()?;
        let sample_format = config.sample_format();
        let config: cpal::StreamConfig = config.into();

        *self.sample_rate.lock().unwrap() = config.sample_rate.0;
        *self.channels.lock().unwrap() = config.channels;
        *self.start_time.lock().unwrap() = Some(Instant::now());

        let is_recording = self.is_recording.clone();
        let is_recording2 = self.is_recording.clone();
        let samples = self.samples.clone();

        let err_fn = |err: cpal::StreamError| {
            log::error!("Stream error: {}", err);
        };

        let stream = match sample_format {
            SampleFormat::F32 => device.build_input_stream(
                &config,
                move |data: &[f32], _: &_| {
                    if !is_recording.load(Ordering::SeqCst) {
                        return;
                    }
                    samples.lock().unwrap().extend_from_slice(data);
                },
                err_fn,
                None,
            )?,
            SampleFormat::I16 => {
                let is_recording_i16 = self.is_recording.clone();
                let samples_i16 = self.samples.clone();
                device.build_input_stream(
                    &config,
                    move |data: &[i16], _: &_| {
                        if !is_recording_i16.load(Ordering::SeqCst) {
                            return;
                        }
                        let floats: Vec<f32> =
                            data.iter().map(|&s| s as f32 / i16::MAX as f32).collect();
                        samples_i16.lock().unwrap().extend_from_slice(&floats);
                    },
                    err_fn,
                    None,
                )?
            }
            format => anyhow::bail!("Unsupported sample format: {:?}", format),
        };

        stream.play()?;
        self.is_recording.store(true, Ordering::SeqCst);

        // Keep the stream alive by sending it via a channel to a dedicated thread.
        // We wrap it in StreamHolder which implements Send (unsafe but correct
        // because only one thread accesses it).
        let holder = StreamHolder(stream);
        std::thread::Builder::new()
            .name("audio-stream-keeper".to_string())
            .spawn(move || {
                let _stream = holder; // Keep stream alive
                while is_recording2.load(Ordering::SeqCst) {
                    std::thread::sleep(std::time::Duration::from_millis(50));
                }
                // Stream drops here, stopping recording
            })?;

        Ok(())
    }

    /// Stop recording and save the audio to WAV files
    pub fn stop(&self, output_dir: &PathBuf) -> Result<(PathBuf, PathBuf)> {
        self.is_recording.store(false, Ordering::SeqCst);

        // Give the stream thread time to notice and drop
        std::thread::sleep(std::time::Duration::from_millis(200));

        let samples = self.samples.lock().unwrap().clone();
        let native_sr = *self.sample_rate.lock().unwrap();
        let channels = *self.channels.lock().unwrap();

        if samples.is_empty() {
            anyhow::bail!("No audio recorded");
        }

        log::info!(
            "Recorded {} samples at {}Hz, {} channels",
            samples.len(),
            native_sr,
            channels
        );

        // Convert to mono if multi-channel
        let mono_samples = if channels > 1 {
            samples
                .chunks(channels as usize)
                .map(|chunk| chunk.iter().sum::<f32>() / channels as f32)
                .collect::<Vec<f32>>()
        } else {
            samples
        };

        // Resample to 16kHz for Whisper/diarization
        let target_sr = 16000u32;
        let resampled = resample(&mono_samples, native_sr, target_sr);

        std::fs::create_dir_all(output_dir)?;

        // Save WAV (for diarization — needs raw PCM)
        let wav_path = output_dir.join("recording.wav");
        save_wav(&resampled, target_sr, &wav_path)?;
        log::info!("Saved WAV: {}", wav_path.display());

        // Encode OGG/Opus for Groq upload (much smaller than WAV)
        let ogg_path = output_dir.join("recording.ogg");
        convert_to_ogg(&resampled, target_sr, &ogg_path)?;

        let ogg_size_mb = std::fs::metadata(&ogg_path)
            .map(|m| m.len() as f64 / (1024.0 * 1024.0))
            .unwrap_or(0.0);
        let wav_size_mb = std::fs::metadata(&wav_path)
            .map(|m| m.len() as f64 / (1024.0 * 1024.0))
            .unwrap_or(0.0);
        log::info!(
            "Audio sizes — WAV: {:.1} MB, OGG/Opus: {:.1} MB (compression ratio: {:.0}x)",
            wav_size_mb,
            ogg_size_mb,
            if ogg_size_mb > 0.0 { wav_size_mb / ogg_size_mb } else { 0.0 }
        );

        Ok((ogg_path, wav_path))
    }

    pub fn is_recording(&self) -> bool {
        self.is_recording.load(Ordering::SeqCst)
    }

    pub fn elapsed_secs(&self) -> f64 {
        self.start_time
            .lock()
            .unwrap()
            .map(|t| t.elapsed().as_secs_f64())
            .unwrap_or(0.0)
    }

    /// Get the current RMS level from accumulated samples
    pub fn current_level(&self) -> f32 {
        let samples = self.samples.lock().unwrap();
        if samples.len() < 1600 {
            return 0.0;
        }
        let recent = &samples[samples.len().saturating_sub(4800)..];
        compute_rms(recent)
    }
}

/// Wrapper to allow cpal::Stream to be sent across threads.
/// SAFETY: We ensure single-thread access via the keeper thread pattern.
/// The field is intentionally "unused" — its purpose is to keep the Stream alive.
#[allow(dead_code)]
struct StreamHolder(cpal::Stream);
unsafe impl Send for StreamHolder {}

fn compute_rms(samples: &[f32]) -> f32 {
    if samples.is_empty() {
        return 0.0;
    }
    let sum_sq: f32 = samples.iter().map(|s| s * s).sum();
    (sum_sq / samples.len() as f32).sqrt()
}

/// Simple linear resampling
fn resample(samples: &[f32], from_sr: u32, to_sr: u32) -> Vec<f32> {
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

/// Encode PCM samples as OGG/Opus using pure Rust (no ffmpeg required).
/// Opus at 32 kbps mono gives ~15× compression over 16-bit PCM WAV.
fn convert_to_ogg(samples: &[f32], sample_rate: u32, ogg_path: &PathBuf) -> Result<()> {
    use audiopus::coder::Encoder;
    use audiopus::{Application, Channels, SampleRate};
    use ogg::writing::{PacketWriteEndInfo, PacketWriter};

    log::info!("Encoding OGG/Opus (libopus statically linked, no ffmpeg)");

    let sr = match sample_rate {
        8_000  => SampleRate::Hz8000,
        12_000 => SampleRate::Hz12000,
        16_000 => SampleRate::Hz16000,
        24_000 => SampleRate::Hz24000,
        _      => SampleRate::Hz48000,
    };

    let mut encoder = Encoder::new(sr, Channels::Mono, Application::Voip)
        .context("Failed to create Opus encoder")?;
    encoder
        .set_bitrate(audiopus::Bitrate::BitsPerSecond(32_000))
        .context("Failed to set Opus bitrate")?;

    // Pre-skip: standard SILK lookahead expressed in 48 kHz samples.
    let pre_skip: u16 = 312;

    let file = std::fs::File::create(ogg_path)
        .with_context(|| format!("Failed to create {}", ogg_path.display()))?;
    let mut pw = PacketWriter::new(std::io::BufWriter::new(file));
    let serial: u32 = 0x4f707573; // "Opus" as u32

    // OpusHead identification header (RFC 7845 §5.1)
    let mut head: Vec<u8> = Vec::with_capacity(19);
    head.extend_from_slice(b"OpusHead");
    head.push(1);                                         // version
    head.push(1);                                         // channels
    head.extend_from_slice(&pre_skip.to_le_bytes());      // pre-skip (48 kHz samples)
    head.extend_from_slice(&sample_rate.to_le_bytes());   // original input sample rate
    head.extend_from_slice(&0u16.to_le_bytes());          // output gain
    head.push(0);                                         // channel mapping family: mono
    pw.write_packet(head, serial, PacketWriteEndInfo::EndPage, 0)
        .context("Failed to write OpusHead")?;

    // OpusTags comment header (RFC 7845 §5.2)
    let vendor = b"sidekick2000";
    let mut tags: Vec<u8> = Vec::new();
    tags.extend_from_slice(b"OpusTags");
    tags.extend_from_slice(&(vendor.len() as u32).to_le_bytes());
    tags.extend_from_slice(vendor);
    tags.extend_from_slice(&0u32.to_le_bytes()); // 0 user comments
    pw.write_packet(tags, serial, PacketWriteEndInfo::EndPage, 0)
        .context("Failed to write OpusTags")?;

    // Audio packets: 20 ms frames.
    // Granule positions are always in 48 kHz samples regardless of input rate.
    let frame_size = (sample_rate / 50) as usize; // e.g. 320 samples at 16 kHz
    let granule_step = 48_000u64 / 50;            // 960 per 20 ms frame at 48 kHz
    let mut granule = pre_skip as u64;
    let mut out_buf = vec![0u8; 4_000];

    let chunks: Vec<&[f32]> = samples.chunks(frame_size).collect();
    let total = chunks.len();

    for (i, chunk) in chunks.into_iter().enumerate() {
        granule += granule_step;

        // f32 → i16, zero-padding the last frame if it's short
        let mut padded = chunk.to_vec();
        padded.resize(frame_size, 0.0);
        let pcm: Vec<i16> = padded
            .iter()
            .map(|&s| (s * i16::MAX as f32).clamp(i16::MIN as f32, i16::MAX as f32) as i16)
            .collect();

        let n = encoder
            .encode(&pcm, &mut out_buf)
            .context("Opus encoding failed")?;

        let is_last = i + 1 == total;
        let end_info = if is_last {
            PacketWriteEndInfo::EndStream
        } else if (i + 1) % 50 == 0 {
            // End a page roughly every second to keep page sizes reasonable
            PacketWriteEndInfo::EndPage
        } else {
            PacketWriteEndInfo::NormalPacket
        };

        pw.write_packet(out_buf[..n].to_vec(), serial, end_info, granule)
            .context("Failed to write Opus packet")?;
    }

    log::info!("OGG/Opus encoding complete: {}", ogg_path.display());
    Ok(())
}

/// Save samples as a 16-bit PCM WAV file
fn save_wav(samples: &[f32], sample_rate: u32, path: &PathBuf) -> Result<()> {
    let spec = WavSpec {
        channels: 1,
        sample_rate,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };

    let mut writer = WavWriter::create(path, spec)?;

    for &sample in samples {
        let s = (sample * i16::MAX as f32).clamp(i16::MIN as f32, i16::MAX as f32) as i16;
        writer.write_sample(s)?;
    }

    writer.finalize()?;
    Ok(())
}
