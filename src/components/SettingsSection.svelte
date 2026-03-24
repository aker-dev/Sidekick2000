<script lang="ts">
  import { settingsState } from "../lib/state.svelte";
  import { LANGUAGES } from "../lib/state.svelte";
  import { open } from "@tauri-apps/plugin-dialog";
  import type { Context } from "../lib/types";
  import Select from "./Select.svelte";

  let activeTab: "keys" | "repo" | "contexts" | "speakers" = $state("keys");

  // Context editing
  let editingContextId: string | null = $state(null);
  let editingLabel: string = $state("");
  let editingContent: string = $state("");

  // New context form
  let newContextLabel: string = $state("");
  let newContextContent: string = $state("");
  let showNewContext: boolean = $state(false);

  // New speaker form
  let newSpeakerName: string = $state("");
  let newSpeakerOrg: string = $state("");

  function generateId(label: string): string {
    return label.toLowerCase().replace(/[^a-z0-9]+/g, "_").replace(/^_|_$/g, "") || Date.now().toString();
  }

  function startEditContext(ctx: Context) {
    editingContextId = ctx.id;
    editingLabel = ctx.label;
    editingContent = ctx.content;
  }

  function saveEditContext() {
    if (!editingContextId) return;
    const idx = settingsState.contexts.findIndex((c) => c.id === editingContextId);
    if (idx >= 0) {
      settingsState.contexts[idx] = {
        id: editingContextId,
        label: editingLabel.trim(),
        content: editingContent,
      };
    }
    editingContextId = null;
  }

  function cancelEditContext() {
    editingContextId = null;
  }

  function deleteContext(id: string) {
    settingsState.contexts = settingsState.contexts.filter((c) => c.id !== id);
  }

  function addContext() {
    if (!newContextLabel.trim()) return;
    const id = generateId(newContextLabel);
    settingsState.contexts.push({
      id,
      label: newContextLabel.trim(),
      content: newContextContent,
    });
    newContextLabel = "";
    newContextContent = "";
    showNewContext = false;
  }

  function addSpeaker() {
    if (!newSpeakerName.trim()) return;
    settingsState.default_speakers.push({
      name: newSpeakerName.trim(),
      organization: newSpeakerOrg.trim(),
    });
    newSpeakerName = "";
    newSpeakerOrg = "";
  }

  function removeSpeaker(i: number) {
    settingsState.default_speakers.splice(i, 1);
  }

  async function browseFolder() {
    const selected = await open({ directory: true, multiple: false });
    if (selected && typeof selected === "string") {
      settingsState.working_folder = selected;
    }
  }

  async function handleSave() {
    await settingsState.save();
  }

  const tabStyle = (tab: string) =>
    `px-3 py-1.5 rounded-md text-sm font-medium cursor-pointer transition-colors ${
      activeTab === tab
        ? "text-white"
        : "opacity-60 hover:opacity-90"
    }`;
</script>

<div class="rounded-lg border" style="background: var(--surface); border-color: var(--border)">
  <!-- Tab bar -->
  <div class="flex gap-1 p-3 border-b" style="border-color: var(--border)">
    <button class={tabStyle("keys")} style={activeTab === "keys" ? "background: var(--accent)" : ""} onclick={() => activeTab = "keys"}>API Keys</button>
    <button class={tabStyle("repo")} style={activeTab === "repo" ? "background: var(--accent)" : ""} onclick={() => activeTab = "repo"}>Repository</button>
    <button class={tabStyle("contexts")} style={activeTab === "contexts" ? "background: var(--accent)" : ""} onclick={() => activeTab = "contexts"}>Contexts</button>
    <button class={tabStyle("speakers")} style={activeTab === "speakers" ? "background: var(--accent)" : ""} onclick={() => activeTab = "speakers"}>Speakers</button>
  </div>

  <div class="p-5">
    <!-- API Keys tab -->
    {#if activeTab === "keys"}
      <div class="space-y-4">
        <div>
          <label class="block text-sm font-medium mb-1" style="color: var(--text-muted)">Groq API Key</label>
          <input
            type="password"
            class="w-full rounded-md px-3 py-2 text-sm border font-mono"
            style="background: var(--bg); border-color: var(--border); color: var(--text)"
            placeholder="gsk_..."
            bind:value={settingsState.groq_api_key}
          />
          <p class="text-xs mt-1 opacity-50">Used for Whisper transcription</p>
        </div>

        <div>
          <label class="block text-sm font-medium mb-2" style="color: var(--text-muted)">Summarization Provider</label>
          <div class="flex gap-3">
            <label class="flex items-center gap-2 cursor-pointer">
              <input
                type="radio"
                name="summarization_provider"
                value="claude"
                bind:group={settingsState.summarization_provider}
              />
              <span class="text-sm">Claude (Anthropic)</span>
            </label>
            <label class="flex items-center gap-2 cursor-pointer">
              <input
                type="radio"
                name="summarization_provider"
                value="together_ai"
                bind:group={settingsState.summarization_provider}
              />
              <span class="text-sm">Together.ai</span>
            </label>
          </div>
        </div>

        {#if settingsState.summarization_provider === "claude"}
          <div>
            <label class="block text-sm font-medium mb-1" style="color: var(--text-muted)">Anthropic API Key</label>
            <input
              type="password"
              class="w-full rounded-md px-3 py-2 text-sm border font-mono"
              style="background: var(--bg); border-color: var(--border); color: var(--text)"
              placeholder="sk-ant-..."
              bind:value={settingsState.anthropic_api_key}
            />
            <p class="text-xs mt-1 opacity-50">Used for Claude summarization</p>
          </div>
        {:else}
          <div>
            <label class="block text-sm font-medium mb-1" style="color: var(--text-muted)">Together.ai API Key</label>
            <input
              type="password"
              class="w-full rounded-md px-3 py-2 text-sm border font-mono"
              style="background: var(--bg); border-color: var(--border); color: var(--text)"
              placeholder="..."
              bind:value={settingsState.together_ai_api_key}
            />
            <p class="text-xs mt-1 opacity-50">Used for Together.ai summarization</p>
          </div>
          <div>
            <label class="block text-sm font-medium mb-1" style="color: var(--text-muted)">Together.ai Model</label>
            <input
              type="text"
              class="w-full rounded-md px-3 py-2 text-sm border font-mono"
              style="background: var(--bg); border-color: var(--border); color: var(--text)"
              placeholder="meta-llama/Llama-3.3-70B-Instruct-Turbo"
              bind:value={settingsState.together_ai_model}
            />
            <p class="text-xs mt-1 opacity-50">Any chat model available on Together.ai</p>
          </div>
        {/if}
      </div>
    {/if}

    <!-- Repository tab -->
    {#if activeTab === "repo"}
      <div class="space-y-4">
        <div>
          <label class="block text-sm font-medium mb-1" style="color: var(--text-muted)">Working Folder</label>
          <div class="flex gap-2">
            <input
              type="text"
              class="flex-1 rounded-md px-3 py-2 text-sm border"
              style="background: var(--bg); border-color: var(--border); color: var(--text)"
              placeholder="/Users/you/MyRepo"
              bind:value={settingsState.working_folder}
            />
            <button
              class="px-3 py-2 rounded-md text-sm font-medium cursor-pointer"
              style="background: var(--surface-hover); color: var(--text)"
              onclick={browseFolder}
            >Browse</button>
          </div>
          <p class="text-xs mt-1 opacity-50">Root of your git repository — meeting notes will be committed here</p>
        </div>
        <div>
          <label class="block text-sm font-medium mb-1" style="color: var(--text-muted)">Meetings Subfolder</label>
          <input
            type="text"
            class="w-full rounded-md px-3 py-2 text-sm border"
            style="background: var(--bg); border-color: var(--border); color: var(--text)"
            placeholder="Meetings"
            bind:value={settingsState.meetings_subfolder}
          />
          <p class="text-xs mt-1 opacity-50">Notes are saved in WorkingFolder/MeetingsSubfolder/</p>
        </div>
        <div>
          <label class="block text-sm font-medium mb-1" style="color: var(--text-muted)">
            GitHub Repo
            <span class="text-xs opacity-60">(action items become issues)</span>
          </label>
          <input
            type="text"
            class="w-full rounded-md px-3 py-2 text-sm border"
            style="background: var(--bg); border-color: var(--border); color: var(--text)"
            placeholder="owner/repo"
            bind:value={settingsState.github_repo}
          />
          <p class="text-xs mt-1 opacity-50">Requires <code>gh</code> CLI installed and authenticated</p>
        </div>
        <div>
          <label class="block text-sm font-medium mb-1" style="color: var(--text-muted)">Default Language</label>
          <Select bind:value={settingsState.default_language}>
            {#each LANGUAGES as lang (lang.code)}
              <option value={lang.code}>{lang.label}</option>
            {/each}
          </Select>
        </div>

        <div>
          <p class="text-sm font-medium mb-2" style="color: var(--text-muted)">Pipeline Steps</p>
          <div class="space-y-2">
            <label class="flex items-center gap-2 cursor-pointer">
              <input type="checkbox" bind:checked={settingsState.enable_summary} />
              <span class="text-sm">Summary</span>
              <span class="text-xs opacity-50">— generate meeting notes with AI</span>
            </label>
            <label class="flex items-center gap-2 cursor-pointer">
              <input type="checkbox" bind:checked={settingsState.enable_git_commit} />
              <span class="text-sm">Git commit</span>
              <span class="text-xs opacity-50">— commit notes to working folder</span>
            </label>
            <label class="flex items-center gap-2 cursor-pointer">
              <input type="checkbox" bind:checked={settingsState.enable_github_issues} />
              <span class="text-sm">Post issues</span>
              <span class="text-xs opacity-50">— create GitHub issues from action items</span>
            </label>
          </div>
        </div>
      </div>
    {/if}

    <!-- Contexts tab -->
    {#if activeTab === "contexts"}
      <div class="space-y-3">
        {#each settingsState.contexts as ctx (ctx.id)}
          {#if editingContextId === ctx.id}
            <!-- Inline edit form -->
            <div class="rounded-md p-3 border space-y-2" style="background: var(--bg); border-color: var(--accent)">
              <input
                type="text"
                class="w-full rounded px-2 py-1.5 text-sm border"
                style="background: var(--surface); border-color: var(--border); color: var(--text)"
                placeholder="Context label"
                bind:value={editingLabel}
              />
              <textarea
                class="w-full rounded px-2 py-1.5 text-sm border"
                style="background: var(--surface); border-color: var(--border); color: var(--text)"
                rows="5"
                placeholder="Context instructions for Claude..."
                bind:value={editingContent}
              ></textarea>
              <div class="flex gap-2">
                <button
                  class="px-3 py-1.5 rounded text-sm font-medium text-white cursor-pointer"
                  style="background: var(--accent)"
                  onclick={saveEditContext}
                >Save</button>
                <button
                  class="px-3 py-1.5 rounded text-sm font-medium cursor-pointer"
                  style="background: var(--surface-hover); color: var(--text)"
                  onclick={cancelEditContext}
                >Cancel</button>
              </div>
            </div>
          {:else}
            <div class="rounded-md px-3 py-2.5 border flex items-center justify-between gap-3" style="background: var(--bg); border-color: var(--border)">
              <div class="min-w-0">
                <p class="text-sm font-medium truncate">{ctx.label}</p>
                <p class="text-xs opacity-50 truncate mt-0.5">{ctx.content.slice(0, 80)}{ctx.content.length > 80 ? "…" : ""}</p>
              </div>
              <div class="flex gap-1 shrink-0">
                <button
                  class="px-2 py-1 rounded text-xs cursor-pointer"
                  style="background: var(--surface-hover); color: var(--text)"
                  onclick={() => startEditContext(ctx)}
                >Edit</button>
                <button
                  class="px-2 py-1 rounded text-xs cursor-pointer"
                  style="background: #451a1a; color: var(--danger)"
                  onclick={() => deleteContext(ctx.id)}
                >Delete</button>
              </div>
            </div>
          {/if}
        {/each}

        {#if settingsState.contexts.length === 0 && !showNewContext}
          <p class="text-sm opacity-50 text-center py-4">No contexts yet. Add one below.</p>
        {/if}

        <!-- New context form -->
        {#if showNewContext}
          <div class="rounded-md p-3 border space-y-2" style="background: var(--bg); border-color: var(--border)">
            <input
              type="text"
              class="w-full rounded px-2 py-1.5 text-sm border"
              style="background: var(--surface); border-color: var(--border); color: var(--text)"
              placeholder="Context label (e.g. Welqin — Product Review)"
              bind:value={newContextLabel}
            />
            <textarea
              class="w-full rounded px-2 py-1.5 text-sm border"
              style="background: var(--surface); border-color: var(--border); color: var(--text)"
              rows="5"
              placeholder="Instructions for Claude when summarizing this meeting type..."
              bind:value={newContextContent}
            ></textarea>
            <div class="flex gap-2">
              <button
                class="px-3 py-1.5 rounded text-sm font-medium text-white cursor-pointer"
                style="background: var(--accent)"
                onclick={addContext}
                disabled={!newContextLabel.trim()}
              >Add Context</button>
              <button
                class="px-3 py-1.5 rounded text-sm font-medium cursor-pointer"
                style="background: var(--surface-hover); color: var(--text)"
                onclick={() => { showNewContext = false; newContextLabel = ""; newContextContent = ""; }}
              >Cancel</button>
            </div>
          </div>
        {:else}
          <button
            class="w-full py-2 rounded-md text-sm border cursor-pointer hover:opacity-80 transition-opacity"
            style="border-color: var(--border); border-style: dashed; color: var(--text-muted)"
            onclick={() => showNewContext = true}
          >+ Add Context</button>
        {/if}
      </div>
    {/if}

    <!-- Default Speakers tab -->
    {#if activeTab === "speakers"}
      <div class="space-y-3">
        {#each settingsState.default_speakers as sp, i (i)}
          <div class="flex items-center gap-2 rounded-md px-3 py-2 border" style="background: var(--bg); border-color: var(--border)">
            <div class="flex-1 min-w-0">
              <span class="text-sm font-medium">{sp.name}</span>
              {#if sp.organization}
                <span class="text-xs opacity-50 ml-2">{sp.organization}</span>
              {/if}
            </div>
            <button
              class="px-2 py-1 rounded text-xs cursor-pointer"
              style="background: #451a1a; color: var(--danger)"
              onclick={() => removeSpeaker(i)}
            >Remove</button>
          </div>
        {/each}

        {#if settingsState.default_speakers.length === 0}
          <p class="text-sm opacity-50 text-center py-2">No default speakers. Add below.</p>
        {/if}

        <div class="flex gap-2">
          <input
            type="text"
            class="flex-1 rounded-md px-3 py-1.5 text-sm border"
            style="background: var(--bg); border-color: var(--border); color: var(--text)"
            placeholder="Name"
            bind:value={newSpeakerName}
            onkeydown={(e: KeyboardEvent) => e.key === "Enter" && addSpeaker()}
          />
          <input
            type="text"
            class="flex-1 rounded-md px-3 py-1.5 text-sm border"
            style="background: var(--bg); border-color: var(--border); color: var(--text)"
            placeholder="Organization"
            bind:value={newSpeakerOrg}
            onkeydown={(e: KeyboardEvent) => e.key === "Enter" && addSpeaker()}
          />
          <button
            class="px-3 py-1.5 rounded-md text-sm font-medium text-white cursor-pointer"
            style="background: var(--accent)"
            onclick={addSpeaker}
          >Add</button>
        </div>
      </div>
    {/if}
  </div>

  <!-- Save footer -->
  <div class="px-5 py-3 border-t flex items-center justify-between gap-3" style="border-color: var(--border)">
    {#if settingsState.saveError}
      <p class="text-xs" style="color: var(--danger)">{settingsState.saveError}</p>
    {:else}
      <p class="text-xs opacity-40">Saved to ~/.sidekick2000/settings.json</p>
    {/if}
    <button
      class="px-4 py-2 rounded-md text-sm font-medium text-white cursor-pointer disabled:opacity-50"
      style="background: var(--accent)"
      onclick={handleSave}
      disabled={settingsState.saving}
    >
      {settingsState.saving ? "Saving…" : "Save Settings"}
    </button>
  </div>
</div>
