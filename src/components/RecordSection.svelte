<script lang="ts">
  import { onMount } from "svelte";
  import { getCurrentWebview } from "@tauri-apps/api/webview";
  import { appState } from "../lib/state.svelte";
  import {
    listInputDevices,
    saveInputDevice,
    startMonitoring,
    stopMonitoring,
    startRecording,
    stopRecording,
    getAudioLevel,
    getElapsed,
    runPipeline,
    onPipelineProgress,
    prepareDroppedAudio,
  } from "../lib/api";
  import type { PipelineConfig } from "../lib/types";
  import AudioMeter from "./AudioMeter.svelte";
  import Select from "./Select.svelte";

  let pollingId: ReturnType<typeof setInterval> | null = null;
  let stopping = $state(false);
  let isDragOver = $state(false);
  let preparingFile = $state("");

  onMount(async () => {
    try {
      appState.inputDevices = await listInputDevices();
    } catch {
      // ignore — device list just stays empty
    }

    // Listen for file drops via Tauri (gives us actual file system paths)
    const webview = getCurrentWebview();
    const unlisten = await webview.onDragDropEvent(async (event) => {
      if (event.payload.type === "over") {
        if (appState.phase === "setup") isDragOver = true;
      } else if (event.payload.type === "leave" || event.payload.type === "cancelled") {
        isDragOver = false;
      } else if (event.payload.type === "drop") {
        isDragOver = false;
        if (appState.phase !== "setup") return;
        const paths = event.payload.paths;
        if (!paths || paths.length === 0) return;
        await handleFileDrop(paths[0]);
      }
    });

    return unlisten;
  });

  // Drive monitor stream while in setup phase; restart when device changes.
  $effect(() => {
    const device = appState.selectedDevice;
    if (appState.phase !== "setup") return;

    startMonitoring(device || undefined).catch(() => {});

    const pollId = setInterval(async () => {
      try {
        appState.audioLevel = await getAudioLevel();
      } catch {
        // ignore
      }
    }, 100);

    return () => {
      clearInterval(pollId);
      stopMonitoring().catch(() => {});
      appState.audioLevel = 0;
    };
  });

  async function handleFileDrop(path: string) {
    preparingFile = path.split("/").pop() ?? path;
    try {
      const [oggPath, wavPath] = await prepareDroppedAudio(path);
      appState.oggPath = oggPath;
      appState.wavPath = wavPath;
      preparingFile = "";
      appState.phase = "processing";
      await startPipeline();
    } catch (e: any) {
      preparingFile = "";
      appState.errorMessage = e.toString();
      appState.phase = "error";
    }
  }

  async function handleRecord() {
    if (appState.phase === "recording") {
      // Stop recording
      stopping = true;
      if (pollingId) {
        clearInterval(pollingId);
        pollingId = null;
      }
      try {
        const [oggPath, wavPath] = await stopRecording();
        appState.oggPath = oggPath;
        appState.wavPath = wavPath;
        stopping = false;
        appState.phase = "processing";
        await startPipeline();
      } catch (e: any) {
        stopping = false;
        appState.errorMessage = e.toString();
        appState.phase = "error";
      }
    } else {
      // Start recording
      try {
        await startRecording(appState.selectedDevice || undefined);
        appState.phase = "recording";
        appState.elapsedSecs = 0;
        appState.audioLevel = 0;

        // Poll for audio level and elapsed time
        pollingId = setInterval(async () => {
          try {
            const [level, elapsed] = await Promise.all([
              getAudioLevel(),
              getElapsed(),
            ]);
            appState.audioLevel = level;
            appState.elapsedSecs = elapsed;
          } catch {
            // ignore polling errors
          }
        }, 100);
      } catch (e: any) {
        appState.errorMessage = e.toString();
        appState.phase = "error";
      }
    }
  }

  async function startPipeline() {
    const unlisten = await onPipelineProgress((step, progress) => {
      appState.pipelineStep = step;
      appState.pipelineProgress = progress;
    });

    try {
      const langMap: Record<string, string> = {
        fr: "French",
        en: "English",
        es: "Spanish",
        de: "German",
        it: "Italian",
        pt: "Portuguese",
      };

      const config: PipelineConfig = {
        context: appState.contextLabel,
        context_content: appState.contextContent,
        meeting_name: appState.meetingName,
        speakers: appState.enabledSpeakers.map((s) => ({
          name: s.name,
          organization: s.organization,
        })),
        language_code: appState.language,
        language_name: langMap[appState.language] || "",
        github_repo: appState.githubRepo,
        output_dir: appState.outputDir,
        working_folder: appState.workingFolder,
        ogg_path: appState.oggPath,
        wav_path: appState.wavPath,
      };

      const result = await runPipeline(config);
      appState.resultPath = result.notes_path;
      appState.createdIssues = result.created_issues;
      appState.phase = "result";
    } catch (e: any) {
      appState.errorMessage = e.toString();
      appState.phase = "error";
    } finally {
      unlisten();
    }
  }
</script>

<section
  class="rounded-lg p-5 border transition-colors"
  style="background: var(--surface); border-color: {isDragOver ? 'var(--accent)' : 'var(--border)'}; outline: {isDragOver ? '2px dashed var(--accent)' : 'none'}; outline-offset: -2px;"
>
  <div class="flex items-center justify-between mb-4">
    <h2 class="text-lg font-semibold">Record</h2>
    {#if appState.phase === "recording"}
      <span class="text-2xl font-mono tabular-nums" style="color: var(--text)">
        {appState.formattedTime}
      </span>
    {/if}
  </div>

  <div class="flex flex-col items-center gap-4">
    <!-- Meeting name -->
    {#if appState.phase === "setup"}
      <div class="w-full">
        <label class="block text-xs mb-1" style="color: var(--text-muted)">
          Meeting name
        </label>
        <input
          type="text"
          class="w-full rounded px-2 py-1 text-sm border"
          style="background: var(--surface-alt, var(--surface)); border-color: var(--border); color: var(--text);"
          placeholder="e.g. Sprint Review"
          bind:value={appState.meetingName}
        />
      </div>
    {/if}

    <!-- Device selector -->
    {#if appState.phase === "setup" && appState.inputDevices.length > 0}
      <div class="w-full">
        <label class="block text-xs mb-1" style="color: var(--text-muted)">
          Input device
        </label>
        <Select
          bind:value={appState.selectedDevice}
          onchange={() => saveInputDevice(appState.selectedDevice).catch(() => {})}
        >
          <option value="">Default</option>
          {#each appState.inputDevices as device}
            <option value={device}>{device}</option>
          {/each}
        </Select>
      </div>
    {/if}

    <!-- Record button -->
    <button
      class="w-20 h-20 rounded-full flex items-center justify-center transition-all border-0"
      class:cursor-pointer={!stopping}
      class:cursor-default={stopping}
      style="background: {appState.phase === 'recording' || stopping ? 'var(--danger)' : 'var(--accent)'}; opacity: {stopping ? 0.6 : 1}; box-shadow: 0 0 {appState.phase === 'recording' && !stopping ? '20px' : '0px'} {appState.phase === 'recording' ? 'var(--danger)' : 'transparent'}"
      onclick={handleRecord}
      disabled={stopping || (appState.phase !== "setup" && appState.phase !== "recording")}
    >
      {#if stopping}
        <!-- Spinner -->
        <svg class="animate-spin" width="28" height="28" viewBox="0 0 24 24" fill="none" stroke="white" stroke-width="2.5">
          <path d="M12 2a10 10 0 0 1 10 10" stroke-linecap="round" />
        </svg>
      {:else if appState.phase === "recording"}
        <!-- Stop icon -->
        <svg width="28" height="28" viewBox="0 0 24 24" fill="white">
          <rect x="6" y="6" width="12" height="12" rx="2" />
        </svg>
      {:else}
        <!-- Mic icon -->
        <svg width="28" height="28" viewBox="0 0 24 24" fill="white">
          <path
            d="M12 1a3 3 0 0 0-3 3v8a3 3 0 0 0 6 0V4a3 3 0 0 0-3-3zM19 10v2a7 7 0 0 1-14 0v-2H3v2a9 9 0 0 0 8 8.94V23h2v-2.06A9 9 0 0 0 21 12v-2h-2z"
          />
        </svg>
      {/if}
    </button>

    {#if stopping}
      <p class="text-sm" style="color: var(--text-muted)">Saving audio…</p>
    {:else if preparingFile}
      <p class="text-xs font-medium" style="color: var(--accent)">
        <svg style="display:inline;vertical-align:-2px" class="animate-spin" width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5"><path d="M12 2a10 10 0 0 1 10 10" stroke-linecap="round" /></svg>
        Preparing {preparingFile}…
      </p>
    {:else if appState.phase === "setup" || appState.phase === "recording"}
      <AudioMeter level={appState.audioLevel} />
      {#if appState.phase === "setup"}
        {#if isDragOver}
          <p class="text-xs font-medium" style="color: var(--accent)">Drop audio file to process</p>
        {:else}
          <p class="text-xs" style="color: var(--text-muted)">Click to record · drop an audio file to process</p>
        {/if}
      {/if}
    {/if}
  </div>
</section>
