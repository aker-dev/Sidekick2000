<script lang="ts">
  import { onMount } from "svelte";
  import { getCurrentWebview } from "@tauri-apps/api/webview";
  import { appState } from "../lib/state.svelte";
  import {
    listInputDevices,
    startMonitoring,
    stopMonitoring,
    startRecording,
    stopRecording,
    getAudioLevels,
    getElapsed,
    runPipeline,
    onPipelineProgress,
    prepareDroppedAudio,
    savePastedImage,
    onLiveSegment,
  } from "../lib/api";
  import type { PipelineConfig } from "../lib/types";
  import AudioMeter from "./AudioMeter.svelte";

  let pollingId: ReturnType<typeof setInterval> | null = null;
  let stopping = $state(false);
  let isDragOver = $state(false);
  let preparingFile = $state("");
  let liveTranscriptEl: HTMLDivElement | undefined = $state();

  // Listen for live-segment events from Tauri backend.
  // Set up once on mount; the callback checks phase reactively.
  onMount(() => {
    let mounted = true;
    let unlisten: (() => void) | undefined;

    onLiveSegment((speaker, segments) => {
      if (appState.phase !== "recording") return;
      for (const seg of segments) {
        appState.liveSegments.push({
          speaker,
          text: seg.text,
          start: seg.start,
          end: seg.end,
        });
      }
      // Auto-scroll to bottom
      requestAnimationFrame(() => {
        if (liveTranscriptEl) {
          liveTranscriptEl.scrollTop = liveTranscriptEl.scrollHeight;
        }
      });
    }).then((fn) => {
      if (mounted) unlisten = fn;
      else fn(); // already unmounted, clean up immediately
    });

    return () => {
      mounted = false;
      unlisten?.();
    };
  });

  onMount(async () => {
    try {
      appState.inputDevices = await listInputDevices();
    } catch {
      // ignore — device list just stays empty
    }

    // Document-level paste listener — section elements don't receive paste
    // events unless focused. Listening on document works regardless of focus.
    const onDocPaste = (e: ClipboardEvent) => handlePaste(e);
    document.addEventListener("paste", onDocPaste);

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

    return () => {
      document.removeEventListener("paste", onDocPaste);
      unlisten();
    };
  });

  // Drive monitor streams on both devices while in setup phase.
  // Restarts automatically when device selection changes.
  $effect(() => {
    const localDevice = appState.selectedDevice;
    const remoteDevice = appState.remoteDevice;
    if (appState.phase !== "setup") return;

    startMonitoring(localDevice || undefined, remoteDevice || undefined).catch(() => {});

    const pollId = setInterval(async () => {
      try {
        const [local, remote] = await getAudioLevels();
        appState.localAudioLevel = local;
        appState.remoteAudioLevel = remote;
      } catch {
        // ignore
      }
    }, 100);

    return () => {
      clearInterval(pollId);
      stopMonitoring().catch(() => {});
      appState.localAudioLevel = 0;
      appState.remoteAudioLevel = 0;
    };
  });

  async function handleFileDrop(path: string) {
    preparingFile = path.split("/").pop() ?? path;
    try {
      const [oggPath] = await prepareDroppedAudio(path);
      appState.localOggPath = oggPath;
      appState.remoteOggPath = "";
      preparingFile = "";
      appState.phase = "processing";
      await startPipeline();
    } catch (e: any) {
      preparingFile = "";
      appState.errorMessage = e.toString();
      appState.phase = "error";
    }
  }

  function formatTimecode(secs: number): string {
    const h = Math.floor(secs / 3600);
    const m = Math.floor((secs % 3600) / 60);
    const s = Math.floor(secs % 60);
    return `${String(h).padStart(2, "0")}:${String(m).padStart(2, "0")}:${String(s).padStart(2, "0")}`;
  }

  async function saveImageBlob(blob: Blob, timecode: number) {
    const ext = blob.type.includes("png") ? "png" : "jpeg";
    const dataUrl = await new Promise<string>((resolve) => {
      const reader = new FileReader();
      reader.onload = () => resolve(reader.result as string);
      reader.readAsDataURL(blob);
    });
    const commaIdx = dataUrl.indexOf(",");
    const data = commaIdx !== -1 ? dataUrl.slice(commaIdx + 1) : dataUrl;
    const path = await savePastedImage(data, ext, timecode);
    appState.pastedImages.push({ dataUrl, timecode, path });
  }

  let isPasting = false;

  async function handlePaste(event: ClipboardEvent) {
    if (appState.phase !== "recording") return;
    if (isPasting) return;
    isPasting = true;
    try {
      const timecode = appState.elapsedSecs;

      // Primary: modern async Clipboard API — more reliable in WKWebView for
      // images copied from native apps (screenshots, etc.)
      try {
        const clipboardItems = await navigator.clipboard.read();
        for (const clipItem of clipboardItems) {
          const imageType = clipItem.types.find((t) => t.startsWith("image/"));
          if (imageType) {
            event.preventDefault();
            const blob = await clipItem.getType(imageType);
            await saveImageBlob(blob, timecode);
            return;
          }
        }
      } catch {
        // Clipboard API unavailable or no permission — fall through to legacy path
      }

      // Fallback: event.clipboardData.items (works for paste from web content)
      const items = event.clipboardData?.items;
      if (!items) return;
      for (const item of Array.from(items)) {
        if (item.type.startsWith("image/")) {
          event.preventDefault();
          const file = item.getAsFile();
          if (!file) continue;
          try {
            await saveImageBlob(file, timecode);
          } catch (e) {
            console.error("Failed to save pasted image:", e);
          }
          break;
        }
      }
    } finally {
      isPasting = false;
    }
  }

  async function handleCancel() {
    if (pollingId) {
      clearInterval(pollingId);
      pollingId = null;
    }
    try {
      await stopRecording();
    } catch {
      // ignore — we're discarding the audio anyway
    }
    appState.reset();
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
        const [localOgg, , remoteOgg] = await stopRecording();
        appState.localOggPath = localOgg;
        appState.remoteOggPath = remoteOgg;
        stopping = false;
        appState.phase = "processing";
        await startPipeline();
      } catch (e: any) {
        stopping = false;
        appState.errorMessage = e.toString();
        appState.phase = "error";
      }
    } else {
      // Start recording on both devices
      try {
        await startRecording(
          appState.selectedDevice || undefined,
          appState.remoteDevice || undefined,
          appState.language || undefined,
        );
        appState.phase = "recording";
        appState.elapsedSecs = 0;
        appState.localAudioLevel = 0;
        appState.remoteAudioLevel = 0;

        pollingId = setInterval(async () => {
          try {
            const [[local, remote], elapsed] = await Promise.all([
              getAudioLevels(),
              getElapsed(),
            ]);
            appState.localAudioLevel = local;
            appState.remoteAudioLevel = remote;
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
        local_ogg_path: appState.localOggPath,
        local_speaker_name: appState.localSpeakerName,
        remote_ogg_path: appState.remoteOggPath,
        remote_speaker_name: appState.remoteSpeakerName,
        image_annotations: appState.pastedImages.map((img) => ({
          path: img.path,
          timecode_secs: img.timecode,
        })),
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
      <div class="flex items-center gap-3">
        <span class="text-2xl font-mono tabular-nums" style="color: var(--text)">
          {appState.formattedTime}
        </span>
        <button
          class="px-3 py-1 rounded text-xs font-medium cursor-pointer"
          style="background: var(--surface-hover); color: var(--text-muted)"
          onclick={handleCancel}
          disabled={stopping}
        >Cancel</button>
      </div>
    {/if}
  </div>

  <div class="flex flex-col items-center gap-4">
    <!-- Meeting name (setup only) -->
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
        <svg class="animate-spin" width="28" height="28" viewBox="0 0 24 24" fill="none" stroke="white" stroke-width="2.5">
          <path d="M12 2a10 10 0 0 1 10 10" stroke-linecap="round" />
        </svg>
      {:else if appState.phase === "recording"}
        <svg width="28" height="28" viewBox="0 0 24 24" fill="white">
          <rect x="6" y="6" width="12" height="12" rx="2" />
        </svg>
      {:else}
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
      <!-- Dual VU meters -->
      <div class="flex gap-8 justify-center">
        <AudioMeter
          level={appState.localAudioLevel}
          label={appState.selectedDevice || appState.localSpeakerName}
        />
        <AudioMeter
          level={appState.remoteAudioLevel}
          label={appState.remoteDevice || appState.remoteSpeakerName}
        />
      </div>

      {#if appState.phase === "recording"}
        <!-- Live transcript -->
        {#if appState.liveSegments.length > 0}
          <div
            bind:this={liveTranscriptEl}
            class="w-full rounded border overflow-y-auto text-xs space-y-1 p-2"
            style="background: var(--surface-alt, var(--surface)); border-color: var(--border); max-height: 160px;"
          >
            {#each appState.liveSegments as seg}
              <div class="flex gap-2">
                <span
                  class="font-medium shrink-0"
                  style="color: {seg.speaker === 'local' ? 'var(--accent)' : 'var(--success)'}; min-width: 3rem;"
                >
                  {seg.speaker === "local" ? appState.localSpeakerName : appState.remoteSpeakerName}
                </span>
                <span style="color: var(--text)">{seg.text.trim()}</span>
              </div>
            {/each}
          </div>
        {:else}
          <p class="text-xs" style="color: var(--text-muted)">Live transcript will appear here…</p>
        {/if}

        <!-- Screenshot thumbnails -->
        {#if appState.pastedImages.length > 0}
          <div class="w-full flex flex-wrap gap-2 mt-1">
            {#each appState.pastedImages as img}
              <div class="relative rounded overflow-hidden border" style="border-color: var(--border); width: 64px; height: 48px; flex-shrink: 0;">
                <img src={img.dataUrl} alt="screenshot" class="w-full h-full object-cover" />
                <span
                  class="absolute bottom-0 left-0 right-0 text-center font-mono"
                  style="font-size: 8px; background: rgba(0,0,0,0.6); color: #fff; padding: 1px 0;"
                >{formatTimecode(img.timecode)}</span>
              </div>
            {/each}
          </div>
        {/if}
        <p class="text-xs" style="color: var(--text-muted)">⌘V to capture a screenshot</p>
      {:else}
        {#if isDragOver}
          <p class="text-xs font-medium" style="color: var(--accent)">Drop audio file to process</p>
        {:else}
          <p class="text-xs" style="color: var(--text-muted)">Click to record · drop an audio file to process</p>
        {/if}
      {/if}
    {/if}
  </div>
</section>
