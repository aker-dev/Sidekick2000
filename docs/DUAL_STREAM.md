# Dual-Stream Local Transcription

## Overview

Sidekick2000 supports dual-stream audio recording with local Whisper transcription. This captures two separate audio sources simultaneously:

- **Local stream**: Your microphone (your voice)
- **Remote stream**: System audio via BlackHole virtual cable (other participants)

Each stream is transcribed independently using whisper.cpp with Metal GPU acceleration, providing perfect speaker separation without any diarization algorithm.

## Prerequisites

### BlackHole Setup

BlackHole is a macOS virtual audio driver that routes system audio to a virtual input device.

1. Install [BlackHole 2ch](https://existential.audio/blackhole/) (the 2-channel version is sufficient)
2. Open **Audio MIDI Setup** (Applications > Utilities)
3. Click the **+** button at the bottom left and select **Create Multi-Output Device**
4. Check both your physical speakers/headphones AND **BlackHole 2ch**
5. Set this Multi-Output Device as your system output (System Settings > Sound > Output)

This routes all system audio to both your speakers and BlackHole simultaneously. Sidekick2000 reads from "BlackHole 2ch" as an input device to capture what others say during calls.

### Whisper Model

The app uses `ggml-large-v3-turbo-q5_0.bin` (~550 MB), downloaded automatically on first use to `~/.sidekick2000/models/`. You can also trigger the download manually via the `download_whisper_model` command.

## How It Works

### Architecture

```
[Mic]               [BlackHole 2ch]
  |                       |
  | cpal                  | cpal
  v                       v
ring buffer           ring buffer
  |                       |
  | worker thread         | worker thread (every 200ms)
  v                       v
mono + resample 16k   mono + resample 16k
  |                       |
  v                       v
VAD (Silero)          VAD (Silero)
  |                       |
  v                       v
WhisperEngine #1      WhisperEngine #2    <-- 2 instances, Metal GPU
  |                       |
  | TranscriptSegment     | TranscriptSegment
  v                       v
emit "live-segment"   emit "live-segment"
  |                       |
  +----------+------------+
             |
             v
    merge (sort by time)
             |
             v
      summarize (Claude)
             |
             v
        export (.md)
```

### Shared Timeline

Both audio streams share a single `Instant` as their t=0 origin, captured just before the cpal streams are started. All segment timestamps are relative to this origin, ensuring correct chronological ordering when merging.

### VAD Chunking Strategy

The Silero Voice Activity Detector processes audio in 32ms chunks (512 samples at 16 kHz). Audio is accumulated and flushed to Whisper when:

- **Silence detected**: >= 300ms of consecutive sub-threshold VAD probability after speech, OR
- **Max duration reached**: >= 10 seconds of accumulated audio

Chunks shorter than 0.3 seconds are discarded as noise.

### Two WhisperEngine Instances

Each stream gets its own WhisperEngine instance rather than sharing one behind a mutex. On Apple Silicon (M4 Pro), Metal can schedule GPU work from multiple threads. A shared mutex would serialize transcription and add latency when both speakers talk simultaneously.

## Tauri Commands

### `list_audio_devices()`

Returns input devices categorized as microphones vs loopback.

```typescript
interface CategorizedDevices {
  microphones: string[];  // Normal mic devices
  loopback: string[];     // BlackHole devices
}

const devices: CategorizedDevices = await invoke('list_audio_devices');
```

### `start_recording(local_device, remote_device)`

Start recording on both devices. In `LocalWhisper` mode, automatically spawns live transcription worker threads.

```typescript
await invoke('start_recording', {
  localDevice: 'MacBook Pro Microphone',
  remoteDevice: 'BlackHole 2ch',
});
```

### `stop_recording()`

Stops recording, finalizes live transcription, saves WAV/OGG files.

```typescript
const [localOgg, localWav, remoteOgg, remoteWav] = await invoke('stop_recording');
```

### `get_model_download_status()`

Check if the Whisper model is downloaded.

```typescript
const status = await invoke('get_model_download_status');
// { downloaded: true, path: "/Users/.../.sidekick2000/models/ggml-large-v3-turbo-q5_0.bin", size_bytes: 574000000 }
```

### `download_whisper_model()`

Trigger model download (emits progress events).

```typescript
const modelPath = await invoke('download_whisper_model');
```

## Tauri Events

### `live-segment`

Emitted during recording whenever a chunk is transcribed. One event per chunk per stream.

```typescript
interface LiveSegmentEvent {
  speaker: 'local' | 'remote';
  segments: Array<{
    id: number;
    start: number;  // seconds from recording start
    end: number;
    text: string;
  }>;
}

listen('live-segment', (event) => {
  const { speaker, segments } = event.payload;
  // Append to live transcript display
});
```

### `model-download-progress`

Emitted during Whisper model download.

```typescript
interface ModelDownloadProgress {
  downloaded: number;  // bytes downloaded so far
  total: number;       // total size in bytes (0 if unknown)
  progress: number;    // 0.0 to 1.0
}

listen('model-download-progress', (event) => {
  const { progress } = event.payload;
  // Update download progress bar
});
```

### `pipeline-progress`

Existing event, unchanged. Emitted during pipeline execution.

```typescript
interface PipelineProgress {
  step: 'transcribing' | 'merging' | 'summarizing' | 'exporting' | 'committing' | 'creating_issues' | 'done';
  progress: number;  // 0.0 to 1.0
}
```

## Settings

The `transcription_mode` field in settings controls which engine is used:

```json
{
  "transcription_mode": "LocalWhisper",
  "default_language": "fr"
}
```

Values: `"LocalWhisper"` (default, offline) or `"Groq"` (cloud, requires API key).

The `default_language` is passed directly to Whisper as an ISO 639-1 code (e.g., `"fr"`, `"en"`).
