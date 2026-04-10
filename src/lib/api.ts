import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import type { PipelineConfig, PipelineResult, PipelineStep, Settings } from "./types";

export async function listInputDevices(): Promise<string[]> {
  return await invoke("list_input_devices_cmd");
}

export async function startMonitoring(localDevice?: string, remoteDevice?: string): Promise<void> {
  await invoke("start_monitoring", {
    localDevice: localDevice ?? null,
    remoteDevice: remoteDevice ?? null,
  });
}

export async function stopMonitoring(): Promise<void> {
  await invoke("stop_monitoring");
}

export async function startRecording(localDevice?: string, remoteDevice?: string, language?: string): Promise<void> {
  await invoke("start_recording", {
    localDevice: localDevice ?? null,
    remoteDevice: remoteDevice ?? null,
    language: language ?? null,
  });
}

/** Returns [localOgg, localWav, remoteOgg, remoteWav]. Remote paths are empty if no remote stream. */
export async function stopRecording(): Promise<[string, string, string, string]> {
  return await invoke("stop_recording");
}

/** Returns [localLevel, remoteLevel] as RMS floats 0..1. */
export async function getAudioLevels(): Promise<[number, number]> {
  return await invoke("get_audio_levels");
}

export async function getElapsed(): Promise<number> {
  return await invoke("get_elapsed");
}

export async function isRecording(): Promise<boolean> {
  return await invoke("is_recording");
}

/** @deprecated Use getAudioLevels() instead. */
export async function getAudioLevel(): Promise<number> {
  const [local] = await getAudioLevels();
  return local;
}

export async function runPipeline(config: PipelineConfig): Promise<PipelineResult> {
  return await invoke("run_pipeline", { config });
}

export async function getDefaultOutputDir(): Promise<string> {
  return await invoke("get_default_output_dir");
}

export async function openFile(path: string): Promise<void> {
  await invoke("open_file", { path });
}

export async function getSettings(): Promise<Settings> {
  return await invoke("get_settings");
}

export async function saveSettings(s: Settings): Promise<void> {
  await invoke("save_settings", { s });
}

export async function saveInputDevice(name: string): Promise<void> {
  await invoke("save_input_device", { name });
}

export function onPipelineProgress(
  callback: (step: PipelineStep, progress: number) => void,
) {
  return listen<{ step: PipelineStep; progress: number }>(
    "pipeline-progress",
    (event) => {
      callback(event.payload.step, event.payload.progress);
    },
  );
}

export async function prepareDroppedAudio(path: string): Promise<[string, string]> {
  return await invoke("prepare_dropped_audio", { path });
}

export async function savePastedImage(
  dataBase64: string,
  extension: string,
  timecodeSecs: number,
): Promise<string> {
  return await invoke("save_pasted_image", { dataBase64, extension, timecodeSecs });
}

export function onAudioLevel(
  callback: (level: number, elapsed: number) => void,
) {
  return listen<{ level: number; elapsed_secs: number }>(
    "audio-level",
    (event) => {
      callback(event.payload.level, event.payload.elapsed_secs);
    },
  );
}

/** Listen for live transcript segments emitted during recording. */
export function onLiveSegment(
  callback: (speaker: string, segments: { text: string; start: number; end: number }[]) => void,
) {
  return listen<{ speaker: string; segments: { text: string; start: number; end: number }[] }>(
    "live-segment",
    (event) => {
      callback(event.payload.speaker, event.payload.segments);
    },
  );
}
