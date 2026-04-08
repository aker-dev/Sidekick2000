use anyhow::{Context, Result};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::SampleFormat;
use hound::{WavSpec, WavWriter};
use std::collections::VecDeque;
use std::path::{Path, PathBuf};
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

/// Categorized audio device lists.
#[derive(Debug, Clone, serde::Serialize)]
pub struct CategorizedDevices {
    /// Normal microphone devices (everything except BlackHole).
    pub microphones: Vec<String>,
    /// Loopback / virtual cable devices (names containing "BlackHole").
    pub loopback: Vec<String>,
}

/// Returns input devices split into microphones and loopback (BlackHole) categories.
pub fn list_audio_devices_categorized() -> CategorizedDevices {
    let all = list_input_devices();
    let mut microphones = Vec::new();
    let mut loopback = Vec::new();

    for name in all {
        if name.contains("BlackHole") {
            loopback.push(name);
        } else {
            microphones.push(name);
        }
    }

    CategorizedDevices {
        microphones,
        loopback,
    }
}

/// Manages audio recording from the default input device.
///
/// Because `cpal::Stream` is not `Send` on macOS, we store the stream
/// behind an `Arc<Mutex<Option<...>>>` and keep it on the creating thread.
/// The stream's data callback pushes samples into a shared buffer.
///
/// The ring buffer (`VecDeque`) accumulates all recorded samples. For batch
/// mode they are consumed at stop time; for live mode a consumer thread can
/// call `drain_samples()` periodically.
pub struct AudioRecorder {
    is_recording: Arc<AtomicBool>,
    /// Ring buffer of raw interleaved samples at native sample rate.
    samples: Arc<Mutex<VecDeque<f32>>>,
    /// Accumulated samples that have already been drained by a live consumer.
    /// Only used during stop to produce the full WAV.
    all_samples: Arc<Mutex<Vec<f32>>>,
    /// Smoothed RMS level updated by both the monitor and recording streams.
    monitor_level: Arc<Mutex<f32>>,
    /// Holds the monitoring stream alive; set to None to stop it.
    monitor_stream: Arc<Mutex<Option<StreamHolder>>>,
    sample_rate: Arc<Mutex<u32>>,
    channels: Arc<Mutex<u16>>,
    /// Shared start time — can be injected externally so both recorders share
    /// a single t=0 origin.
    start_time: Arc<Mutex<Option<Instant>>>,
    /// Total number of mono samples pushed (at native rate), used to compute
    /// the current time offset for live transcription.
    total_samples_pushed: Arc<Mutex<u64>>,
}

/// Capacity of the ring buffer: 60 seconds at 48 kHz stereo (worst case).
const RING_BUFFER_CAPACITY: usize = 48_000 * 2 * 60;

impl AudioRecorder {
    pub fn new() -> Self {
        Self {
            is_recording: Arc::new(AtomicBool::new(false)),
            samples: Arc::new(Mutex::new(VecDeque::with_capacity(RING_BUFFER_CAPACITY))),
            all_samples: Arc::new(Mutex::new(Vec::new())),
            monitor_level: Arc::new(Mutex::new(0.0)),
            monitor_stream: Arc::new(Mutex::new(None)),
            sample_rate: Arc::new(Mutex::new(44100)),
            channels: Arc::new(Mutex::new(1)),
            start_time: Arc::new(Mutex::new(None)),
            total_samples_pushed: Arc::new(Mutex::new(0)),
        }
    }

    /// Create a recorder that shares the given start_time with another recorder.
    pub fn new_with_shared_start_time(start_time: Arc<Mutex<Option<Instant>>>) -> Self {
        Self {
            start_time,
            ..Self::new()
        }
    }

    /// Get a clone of the shared start_time Arc for sharing with another recorder.
    pub fn shared_start_time(&self) -> Arc<Mutex<Option<Instant>>> {
        self.start_time.clone()
    }

    /// Open a lightweight input stream just for level monitoring (no sample accumulation).
    pub fn start_monitor(&self, device_name: Option<String>) -> Result<()> {
        // Drop any existing monitor stream first.
        self.stop_monitor();

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

        log::info!("Starting monitor stream for: {}", device.name().unwrap_or_default());

        let config = device.default_input_config()?;
        let sample_format = config.sample_format();
        let config: cpal::StreamConfig = config.into();

        let level_f32 = self.monitor_level.clone();
        let level_i16 = self.monitor_level.clone();

        let stream = match sample_format {
            SampleFormat::F32 => device.build_input_stream(
                &config,
                move |data: &[f32], _: &_| {
                    *level_f32.lock().unwrap() = compute_rms(data);
                },
                |err| log::error!("Monitor stream error: {}", err),
                None,
            )?,
            SampleFormat::I16 => device.build_input_stream(
                &config,
                move |data: &[i16], _: &_| {
                    let floats: Vec<f32> =
                        data.iter().map(|&s| s as f32 / i16::MAX as f32).collect();
                    *level_i16.lock().unwrap() = compute_rms(&floats);
                },
                |err| log::error!("Monitor stream error: {}", err),
                None,
            )?,
            format => anyhow::bail!("Unsupported sample format: {:?}", format),
        };

        stream.play()?;
        *self.monitor_stream.lock().unwrap() = Some(StreamHolder(stream));
        Ok(())
    }

    /// Stop the monitor stream and reset the level.
    pub fn stop_monitor(&self) {
        *self.monitor_stream.lock().unwrap() = None;
        *self.monitor_level.lock().unwrap() = 0.0;
    }

    /// Start recording from the specified input device, or the default if `device_name` is None.
    /// The stream is kept alive by a dedicated non-Send thread
    /// spawned via `dispatch` on macOS.
    pub fn start(&self, device_name: Option<String>) -> Result<()> {
        if self.is_recording.load(Ordering::SeqCst) {
            anyhow::bail!("Already recording");
        }

        // Release the monitor stream so the device is free for recording.
        self.stop_monitor();

        // Clear previous samples
        self.samples.lock().unwrap().clear();
        self.all_samples.lock().unwrap().clear();
        *self.total_samples_pushed.lock().unwrap() = 0;

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

        // Set start_time only if not already set (shared start_time case)
        {
            let mut st = self.start_time.lock().unwrap();
            if st.is_none() {
                *st = Some(Instant::now());
            }
        }

        let is_recording = self.is_recording.clone();
        let is_recording2 = self.is_recording.clone();
        let samples = self.samples.clone();
        let total_pushed = self.total_samples_pushed.clone();
        let level_f32 = self.monitor_level.clone();
        let level_i16 = self.monitor_level.clone();

        let stream = match sample_format {
            SampleFormat::F32 => device.build_input_stream(
                &config,
                move |data: &[f32], _: &_| {
                    if !is_recording.load(Ordering::SeqCst) {
                        return;
                    }
                    let mut buf = samples.lock().unwrap();
                    buf.extend(data.iter());
                    *total_pushed.lock().unwrap() += data.len() as u64;
                    *level_f32.lock().unwrap() = compute_rms(data);
                },
                |err| log::error!("Stream error: {}", err),
                None,
            )?,
            SampleFormat::I16 => {
                let is_recording_i16 = self.is_recording.clone();
                let samples_i16 = self.samples.clone();
                let total_pushed_i16 = self.total_samples_pushed.clone();
                device.build_input_stream(
                    &config,
                    move |data: &[i16], _: &_| {
                        if !is_recording_i16.load(Ordering::SeqCst) {
                            return;
                        }
                        let floats: Vec<f32> =
                            data.iter().map(|&s| s as f32 / i16::MAX as f32).collect();
                        *level_i16.lock().unwrap() = compute_rms(&floats);
                        let mut buf = samples_i16.lock().unwrap();
                        buf.extend(floats.iter());
                        *total_pushed_i16.lock().unwrap() += floats.len() as u64;
                    },
                    |err| log::error!("Stream error: {}", err),
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

    /// Stop recording and save audio files using `prefix` as the base filename.
    /// Produces `{prefix}.ogg` (for Groq) and `{prefix}.wav` (raw PCM).
    pub fn stop(&self, output_dir: &PathBuf, prefix: &str) -> Result<(PathBuf, PathBuf)> {
        self.is_recording.store(false, Ordering::SeqCst);

        // Give the stream thread time to notice and drop
        std::thread::sleep(std::time::Duration::from_millis(200));

        // Collect all samples: previously drained + still in ring buffer
        let mut all = self.all_samples.lock().unwrap().clone();
        let remaining: Vec<f32> = self.samples.lock().unwrap().drain(..).collect();
        all.extend_from_slice(&remaining);

        let native_sr = *self.sample_rate.lock().unwrap();
        let channels = *self.channels.lock().unwrap();

        if all.is_empty() {
            anyhow::bail!("No audio recorded");
        }

        log::info!(
            "[{}] Recorded {} samples at {}Hz, {} channels",
            prefix, all.len(), native_sr, channels
        );

        // Alias for the rest of the method
        let samples = all;

        // Convert to mono if multi-channel
        let mono_samples = if channels > 1 {
            samples
                .chunks(channels as usize)
                .map(|chunk| chunk.iter().sum::<f32>() / channels as f32)
                .collect::<Vec<f32>>()
        } else {
            samples
        };

        // Resample to 16 kHz for Whisper
        let target_sr = 16000u32;
        let resampled = resample(&mono_samples, native_sr, target_sr);

        std::fs::create_dir_all(output_dir)?;

        let wav_path = output_dir.join(format!("{}.wav", prefix));
        save_wav(&resampled, target_sr, &wav_path)?;
        log::info!("Saved WAV: {}", wav_path.display());

        let ogg_path = output_dir.join(format!("{}.ogg", prefix));
        convert_to_ogg(&resampled, target_sr, &ogg_path)?;

        let ogg_size_mb = std::fs::metadata(&ogg_path)
            .map(|m| m.len() as f64 / (1024.0 * 1024.0))
            .unwrap_or(0.0);
        log::info!(
            "[{}] OGG/Opus: {:.1} MB",
            prefix, ogg_size_mb
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

    /// Get the current RMS level (updated by both the monitor and recording streams).
    pub fn current_level(&self) -> f32 {
        *self.monitor_level.lock().unwrap()
    }

    /// Returns true if any audio samples have been accumulated (i.e. recording was started).
    pub fn has_samples(&self) -> bool {
        !self.samples.lock().unwrap().is_empty()
            || !self.all_samples.lock().unwrap().is_empty()
            || *self.total_samples_pushed.lock().unwrap() > 0
    }

    /// Drain all samples currently in the ring buffer.
    /// The drained samples are also appended to `all_samples` so that `stop()`
    /// can still produce the full WAV.
    /// Returns (drained_samples, native_sample_rate, native_channels).
    #[allow(dead_code)]
    pub fn drain_samples(&self) -> (Vec<f32>, u32, u16) {
        let mut buf = self.samples.lock().unwrap();
        let drained: Vec<f32> = buf.drain(..).collect();
        drop(buf);

        // Keep a copy for the final WAV
        self.all_samples.lock().unwrap().extend_from_slice(&drained);

        let sr = *self.sample_rate.lock().unwrap();
        let ch = *self.channels.lock().unwrap();
        (drained, sr, ch)
    }

    /// Returns the native sample rate of the current recording.
    #[allow(dead_code)]
    pub fn native_sample_rate(&self) -> u32 {
        *self.sample_rate.lock().unwrap()
    }

    /// Returns the number of native channels.
    #[allow(dead_code)]
    pub fn native_channels(&self) -> u16 {
        *self.channels.lock().unwrap()
    }

    // --- Arc ref accessors for live transcription worker threads ---

    pub fn samples_ref(&self) -> Arc<Mutex<VecDeque<f32>>> {
        self.samples.clone()
    }

    pub fn all_samples_ref(&self) -> Arc<Mutex<Vec<f32>>> {
        self.all_samples.clone()
    }

    pub fn sample_rate_ref(&self) -> Arc<Mutex<u32>> {
        self.sample_rate.clone()
    }

    pub fn channels_ref(&self) -> Arc<Mutex<u16>> {
        self.channels.clone()
    }

    #[allow(dead_code)]
    pub fn is_recording_ref(&self) -> Arc<AtomicBool> {
        self.is_recording.clone()
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

/// Decode any audio file supported by symphonia into interleaved f32 samples.
/// Returns (samples, sample_rate, channel_count).
fn decode_audio_file(path: &Path) -> Result<(Vec<f32>, u32, u16)> {
    use symphonia::core::audio::SampleBuffer;
    use symphonia::core::codecs::DecoderOptions;
    use symphonia::core::errors::Error as SymphoniaError;
    use symphonia::core::formats::FormatOptions;
    use symphonia::core::io::MediaSourceStream;
    use symphonia::core::meta::MetadataOptions;
    use symphonia::core::probe::Hint;

    let file = std::fs::File::open(path)
        .with_context(|| format!("Cannot open file: {}", path.display()))?;
    let mss = MediaSourceStream::new(Box::new(file), Default::default());

    let mut hint = Hint::new();
    if let Some(ext) = path.extension().and_then(|e: &std::ffi::OsStr| e.to_str()) {
        hint.with_extension(ext);
    }

    let probed = symphonia::default::get_probe()
        .format(&hint, mss, &FormatOptions::default(), &MetadataOptions::default())
        .context("Unsupported or unrecognized audio format")?;

    let mut format = probed.format;

    let track = format
        .default_track()
        .context("No audio track found in file")?;
    let track_id = track.id;
    let sample_rate = track.codec_params.sample_rate.unwrap_or(44100);
    let channels = track
        .codec_params
        .channels
        .map(|c| c.count() as u16)
        .unwrap_or(1);

    let mut decoder = symphonia::default::get_codecs()
        .make(&track.codec_params, &DecoderOptions::default())
        .context("Unsupported audio codec")?;

    let mut all_samples: Vec<f32> = Vec::new();

    loop {
        let packet = match format.next_packet() {
            Ok(p) => p,
            Err(SymphoniaError::IoError(ref e))
                if e.kind() == std::io::ErrorKind::UnexpectedEof =>
            {
                break;
            }
            Err(SymphoniaError::ResetRequired) => break,
            Err(e) => anyhow::bail!("Error reading audio packet: {}", e),
        };

        if packet.track_id() != track_id {
            continue;
        }

        match decoder.decode(&packet) {
            Ok(decoded) => {
                let spec = *decoded.spec();
                let duration = decoded.capacity() as u64;
                let mut sample_buf = SampleBuffer::<f32>::new(duration, spec);
                sample_buf.copy_interleaved_ref(decoded);
                all_samples.extend_from_slice(sample_buf.samples());
            }
            Err(SymphoniaError::DecodeError(e)) => {
                log::warn!("Decode error (skipping packet): {}", e);
                continue;
            }
            Err(e) => anyhow::bail!("Fatal decode error: {}", e),
        }
    }

    anyhow::ensure!(!all_samples.is_empty(), "No audio data decoded from file");

    Ok((all_samples, sample_rate, channels))
}

/// Decode any supported audio file, convert to mono 16 kHz, and write
/// both an OGG/Opus file (for Groq transcription) and a WAV file (for diarization).
/// Returns (ogg_path, wav_path).
pub fn prepare_audio_file(input_path: &Path, output_dir: &Path) -> Result<(PathBuf, PathBuf)> {
    log::info!("Preparing audio file: {}", input_path.display());

    let (samples, sample_rate, channels) = decode_audio_file(input_path)?;
    log::info!(
        "Decoded {} samples at {} Hz, {} ch",
        samples.len(),
        sample_rate,
        channels
    );

    // Convert to mono
    let mono: Vec<f32> = if channels > 1 {
        samples
            .chunks(channels as usize)
            .map(|chunk| chunk.iter().sum::<f32>() / channels as f32)
            .collect()
    } else {
        samples
    };

    // Resample to 16 kHz (required by both Whisper and diarization)
    let target_sr = 16_000u32;
    let resampled = resample(&mono, sample_rate, target_sr);

    std::fs::create_dir_all(output_dir)?;

    let stem = input_path
        .file_stem()
        .unwrap_or_default()
        .to_string_lossy();
    let wav_path = output_dir.join(format!("{}_prepared.wav", stem));
    let ogg_path = output_dir.join(format!("{}_prepared.ogg", stem));

    save_wav(&resampled, target_sr, &wav_path)?;
    convert_to_ogg(&resampled, target_sr, &ogg_path)?;

    log::info!(
        "Prepared audio — OGG: {}, WAV: {}",
        ogg_path.display(),
        wav_path.display()
    );
    Ok((ogg_path, wav_path))
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
