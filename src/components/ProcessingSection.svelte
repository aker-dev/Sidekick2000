<script lang="ts">
  import { appState } from "../lib/state.svelte";

  // Choose the transcription step label based on whether live segments were reused
  const isLiveReuse = $derived(
    appState.pipelineStep === "reusing_live" ||
    (appState.pipelineStep !== "transcribing" && appState.liveSegments.length > 0)
  );

  const baseSteps = $derived([
    { key: isLiveReuse ? "reusing_live" : "transcribing", label: isLiveReuse ? "Using live transcript" : "Transcribing" },
    { key: "diarizing", label: "Identifying Speakers" },
    { key: "merging", label: "Merging" },
    { key: "summarizing", label: "Summarizing" },
    { key: "exporting", label: "Exporting" },
  ]);

  let steps = $derived.by(() => {
    const s = [...baseSteps];
    if (appState.workingFolder) s.push({ key: "committing", label: "Committing to Git" });
    if (appState.githubRepo) s.push({ key: "creating_issues", label: "Creating GitHub Issues" });
    return s;
  });

  function stepStatus(stepKey: string): "done" | "active" | "pending" {
    if (appState.pipelineStep === "done") return "done";
    const currentIdx = steps.findIndex((s) => s.key === appState.pipelineStep);
    const stepIdx = steps.findIndex((s) => s.key === stepKey);
    if (stepIdx < currentIdx) return "done";
    if (stepIdx === currentIdx) return "active";
    return "pending";
  }
</script>

<section
  class="rounded-lg p-5 border"
  style="background: var(--surface); border-color: var(--border)"
>
  <h2 class="text-lg font-semibold mb-4">Processing</h2>

  <!-- Progress bar -->
  <div
    class="w-full h-2 rounded-full mb-6 overflow-hidden"
    style="background: var(--border)"
  >
    <div
      class="h-full rounded-full transition-all duration-500"
      style="width: {appState.pipelineProgress * 100}%; background: var(--accent)"
    ></div>
  </div>

  <!-- Step indicators -->
  <div class="space-y-2">
    {#each steps as step}
      {@const status = stepStatus(step.key)}
      <div class="flex items-center gap-3">
        <!-- Status icon -->
        <div class="w-5 h-5 flex items-center justify-center">
          {#if status === "done"}
            <svg
              width="18"
              height="18"
              viewBox="0 0 24 24"
              fill="none"
              stroke="var(--success)"
              stroke-width="3"
            >
              <polyline points="20 6 9 17 4 12" />
            </svg>
          {:else if status === "active"}
            <div
              class="w-4 h-4 rounded-full border-2 border-t-transparent animate-spin"
              style="border-color: var(--accent); border-top-color: transparent"
            ></div>
          {:else}
            <div
              class="w-3 h-3 rounded-full"
              style="background: var(--border)"
            ></div>
          {/if}
        </div>

        <span
          class="text-sm"
          style="color: {status === 'active'
            ? 'var(--text)'
            : status === 'done'
              ? 'var(--success)'
              : 'var(--text-muted)'}"
        >
          {step.label}
        </span>
      </div>
    {/each}
  </div>
</section>
