# Sidekick2000

A macOS desktop app that records meetings, transcribes them, and produces structured notes — with action items pushed directly as GitHub issues and notes committed to a git repository.

Built with [Tauri](https://tauri.app) + Svelte 5 on the frontend, Rust on the backend.

---

## What it does

1. **Records** system and microphone audio simultaneously
2. **Transcribes** using [Groq Whisper](https://groq.com) (fast, multilingual)
3. **Diarizes** — identifies which speaker said what using local speaker separation
4. **Summarizes** with Claude Sonnet, using a context you define (meeting type, participants, domain vocabulary)
5. **Exports** structured Markdown notes (`YYYY-MM-DD_HHmm_Context.md`) to a configurable folder
6. **Commits** the notes to a git repository automatically
7. **Creates GitHub issues** for every action item extracted from the notes

---

## Output format

Each meeting produces two files:

```
Meetings/
  2026-03-20_1430_welqin.md          ← structured notes
  2026-03-20_1430_welqin_transcript.md ← raw diarized transcript
```

The notes follow a consistent structure:

```markdown
## Participants
## Summary
## Key Discussion Points
## Decisions Made
## Action Items
- [ ] **David**: Review the API design document
- [ ] **Yannick**: Set up CI pipeline for staging
```

Action items are automatically created as GitHub issues with the `meeting-action` label.

---

## Setup

### Requirements

- macOS (Apple Silicon or Intel)
- [Groq API key](https://console.groq.com) — for Whisper transcription
- [Anthropic API key](https://console.anthropic.com) — for Claude summarization
- [`gh` CLI](https://cli.github.com) installed and authenticated — for GitHub issues
- `git` — for committing notes

### Install dependencies

```bash
npm install
```

### Configure

Launch the app and click the **gear icon** in the top-right corner. All settings are stored in `~/.sidekick2000/settings.json`.

| Tab | What to configure |
|-----|-------------------|
| **API Keys** | Groq and Anthropic API keys |
| **Repository** | Working folder (git root), meetings subfolder, GitHub repo (`owner/repo`), default language |
| **Contexts** | Meeting context templates — instructions that shape how Claude summarizes each meeting type |
| **Speakers** | Default speaker list pre-loaded at startup |

> API keys set in Settings take priority over environment variables. You can still use a `.env` file as fallback.

### Dev mode

```bash
npm run tauri dev
```

### Build

```bash
npm run tauri build
```

---

## Contexts

Contexts are the core of Sidekick2000's flexibility. Each context is a Markdown document that gives Claude background knowledge about a meeting type: who the participants are, domain vocabulary, and how to structure the notes.

Examples of contexts you might create:

- **General** — neutral instructions, works for any meeting
- **Product review** — focus on decisions, feature requests, backlog items
- **Client call** — highlight commitments, risks, next steps
- **Training session** — track exercises, Q&A, shortcuts mentioned

Contexts are managed entirely in the Settings UI (no external files needed).

---

## Pipeline

```
Record audio (OGG + WAV)
       │
       ▼
Transcribe ──────────────┐   (Groq Whisper, async)
                         │
Diarize ─────────────────┘   (local speaker separation, parallel)
       │
       ▼
Merge transcript + diarization
       │
       ▼
Summarize with Claude Sonnet 4.6
       │
       ▼
Export  YYYY-MM-DD_HHmm_Context.md
       │
       ▼
Git commit  (if working folder configured)
       │
       ▼
Create GitHub issues  (if repo configured)
```

---

## Settings file

`~/.sidekick2000/settings.json` — created automatically on first save.

```json
{
  "groq_api_key": "gsk_...",
  "anthropic_api_key": "sk-ant-...",
  "working_folder": "/Users/you/my-repo",
  "github_repo": "owner/repo",
  "meetings_subfolder": "Meetings",
  "default_language": "fr",
  "default_speakers": [
    { "name": "Alice", "organization": "Acme" }
  ],
  "contexts": [
    {
      "id": "general",
      "label": "General",
      "content": "Be factual. Group by theme. Use professional tone."
    }
  ]
}
```

---

## Tech stack

| Layer | Technology |
|-------|-----------|
| UI | Svelte 5, Tailwind CSS 4 |
| Desktop shell | Tauri 2 |
| Backend | Rust (async with Tokio) |
| Transcription | Groq Whisper API (`whisper-large-v3-turbo`) |
| Summarization | Anthropic Claude Sonnet 4.6 |
| Speaker diarization | Local (custom MFCC + clustering) |
| Audio capture | CPAL |
| GitHub integration | `gh` CLI |
