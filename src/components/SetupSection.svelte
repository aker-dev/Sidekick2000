<script lang="ts">
  import { appState, LANGUAGES } from "../lib/state.svelte";
  import SpeakerChip from "./SpeakerChip.svelte";

  let newName = $state("");
  let newOrg = $state("");

  function addSpeaker() {
    if (!newName.trim()) return;
    appState.speakers.push({
      name: newName.trim(),
      organization: newOrg.trim(),
      enabled: true,
    });
    newName = "";
    newOrg = "";
  }

  function removeSpeaker(index: number) {
    appState.speakers.splice(index, 1);
  }

  function toggleSpeaker(index: number) {
    appState.speakers[index].enabled = !appState.speakers[index].enabled;
  }
</script>

<section
  class="rounded-lg p-5 border"
  style="background: var(--surface); border-color: var(--border)"
  class:opacity-50={appState.phase !== "setup"}
>
  <h2 class="text-lg font-semibold mb-4">Setup</h2>

  <!-- Context -->
  <div class="mb-4">
    <label for="context-select" class="block text-sm font-medium mb-1" style="color: var(--text-muted)">
      Context
    </label>
    {#if appState.contexts.length > 0}
      <select
        id="context-select"
        class="w-full rounded-md px-3 py-2 text-sm border"
        style="background: var(--bg); border-color: var(--border); color: var(--text)"
        bind:value={appState.selectedContextId}
        disabled={appState.phase !== "setup"}
      >
        {#each appState.contexts as ctx (ctx.id)}
          <option value={ctx.id}>{ctx.label}</option>
        {/each}
        <option value="custom">Custom...</option>
      </select>
    {:else}
      <p class="text-xs rounded-md px-3 py-2 border" style="border-color: var(--border); color: var(--text-muted)">
        No contexts configured. Open Settings to add contexts.
      </p>
    {/if}
    {#if appState.selectedContextId === "custom"}
      <textarea
        class="w-full mt-2 rounded-md px-3 py-2 text-sm border"
        style="background: var(--bg); border-color: var(--border); color: var(--text)"
        rows="3"
        placeholder="Enter custom context for summarization..."
        bind:value={appState.customContext}
        disabled={appState.phase !== "setup"}
      ></textarea>
    {/if}
  </div>

  <!-- Language -->
  <div class="mb-4">
    <label for="language-select" class="block text-sm font-medium mb-1" style="color: var(--text-muted)">
      Language
    </label>
    <select
      id="language-select"
      class="w-full rounded-md px-3 py-2 text-sm border"
      style="background: var(--bg); border-color: var(--border); color: var(--text)"
      bind:value={appState.language}
      disabled={appState.phase !== "setup"}
    >
      {#each LANGUAGES as lang (lang.code)}
        <option value={lang.code}>{lang.label}</option>
      {/each}
    </select>
  </div>

  <!-- Speakers -->
  <div class="mb-4">
    <span class="block text-sm font-medium mb-2" style="color: var(--text-muted)">
      Speakers
    </span>
    <div class="flex flex-wrap gap-2 mb-3">
      {#each appState.speakers as speaker, i (speaker.name + speaker.organization)}
        <SpeakerChip
          name={speaker.name}
          organization={speaker.organization}
          enabled={speaker.enabled}
          ontoggle={() => toggleSpeaker(i)}
          onremove={() => removeSpeaker(i)}
          disabled={appState.phase !== "setup"}
        />
      {/each}
    </div>

    <!-- Add speaker form -->
    {#if appState.phase === "setup"}
      <div class="flex gap-2">
        <input
          type="text"
          class="flex-1 rounded-md px-3 py-1.5 text-sm border"
          style="background: var(--bg); border-color: var(--border); color: var(--text)"
          placeholder="Name"
          bind:value={newName}
          onkeydown={(e: KeyboardEvent) => e.key === "Enter" && addSpeaker()}
        />
        <input
          type="text"
          class="flex-1 rounded-md px-3 py-1.5 text-sm border"
          style="background: var(--bg); border-color: var(--border); color: var(--text)"
          placeholder="Organization"
          bind:value={newOrg}
          onkeydown={(e: KeyboardEvent) => e.key === "Enter" && addSpeaker()}
        />
        <button
          class="px-3 py-1.5 rounded-md text-sm font-medium text-white cursor-pointer"
          style="background: var(--accent)"
          onclick={addSpeaker}
        >
          Add
        </button>
      </div>
    {/if}
  </div>

  <!-- Output dir (read-only display, configurable via Settings) -->
  <div>
    <p class="block text-sm font-medium mb-1" style="color: var(--text-muted)">
      Output Directory
      <span class="text-xs opacity-60">(configure in Settings)</span>
    </p>
    <p
      class="w-full rounded-md px-3 py-2 text-sm border truncate"
      style="background: var(--bg); border-color: var(--border); color: var(--text-muted)"
      title={appState.outputDir}
    >
      {appState.outputDir || "Not configured"}
    </p>
  </div>
</section>
