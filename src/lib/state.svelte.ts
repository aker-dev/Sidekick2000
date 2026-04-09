import type { Speaker, AppPhase, PipelineStep, CreatedIssue, Context, Settings, ImageAnnotation, LiveSegment } from "./types";
import { getSettings, saveSettings, getDefaultOutputDir } from "./api";

export const LANGUAGES = [
  { code: "fr", label: "French" },
  { code: "en", label: "English" },
  { code: "es", label: "Spanish" },
  { code: "de", label: "German" },
  { code: "it", label: "Italian" },
  { code: "pt", label: "Portuguese" },
  { code: "", label: "Auto-detect" },
];

class AppState {
  phase: AppPhase = $state("setup");

  // Setup
  selectedContextId: string = $state("default");
  customContext: string = $state("");
  language: string = $state("fr");
  outputDir: string = $state("");
  speakers: Speaker[] = $state([]);

  // Contexts from settings
  contexts: Context[] = $state([]);

  // GitHub repo from settings
  githubRepo: string = $state("");

  // Working folder from settings (for git commit)
  workingFolder: string = $state("");

  // Meeting name
  meetingName: string = $state("");

  // Recording
  localAudioLevel: number = $state(0);
  remoteAudioLevel: number = $state(0);
  elapsedSecs: number = $state(0);
  selectedDevice: string = $state(""); // local mic device
  remoteDevice: string = $state("");
  localSpeakerName: string = $state("Local");
  remoteSpeakerName: string = $state("Remote");
  inputDevices: string[] = $state([]);

  // Processing
  pipelineStep: PipelineStep = $state("transcribing");
  pipelineProgress: number = $state(0);

  // Result
  resultPath: string = $state("");
  errorMessage: string = $state("");

  // GitHub
  createdIssues: CreatedIssue[] = $state([]);

  // Recording file paths (set after stop)
  localOggPath: string = $state("");
  remoteOggPath: string = $state("");

  // Images pasted during recording
  pastedImages: { dataUrl: string; timecode: number; path: string }[] = $state([]);

  // Live transcript segments received during recording
  liveSegments: LiveSegment[] = $state([]);

  get enabledSpeakers(): Speaker[] {
    return this.speakers.filter((s) => s.enabled);
  }

  get formattedTime(): string {
    const mins = Math.floor(this.elapsedSecs / 60);
    const secs = Math.floor(this.elapsedSecs % 60);
    return `${String(mins).padStart(2, "0")}:${String(secs).padStart(2, "0")}`;
  }

  get selectedContext(): Context | undefined {
    return this.contexts.find((c) => c.id === this.selectedContextId);
  }

  get contextContent(): string {
    if (this.selectedContextId === "custom") return this.customContext;
    return this.selectedContext?.content ?? "";
  }

  get contextLabel(): string {
    if (this.selectedContextId === "custom") return "custom";
    return this.selectedContext?.label ?? this.selectedContextId;
  }

  async loadFromSettings() {
    try {
      const s = await getSettings();
      this.applySettings(s);
    } catch (e) {
      console.error("Failed to load settings:", e);
    }
    // Always refresh output dir from backend (respects settings)
    try {
      this.outputDir = await getDefaultOutputDir();
    } catch (e) {
      console.error("Failed to get output dir:", e);
    }
  }

  applySettings(s: Settings) {
    this.contexts = s.contexts;
    this.githubRepo = s.github_repo;
    this.workingFolder = s.working_folder;
    this.language = s.default_language || "fr";
    this.selectedDevice = s.default_input_device || "";
    this.remoteDevice = s.remote_device || "";
    this.localSpeakerName = s.local_speaker_name || "Local";
    this.remoteSpeakerName = s.remote_speaker_name || "Remote";
    this.speakers = s.default_speakers.map((sp) => ({ ...sp, enabled: true }));
    // Select first context by default
    if (s.contexts.length > 0 && this.selectedContextId === "default") {
      this.selectedContextId = s.contexts[0].id;
    }
  }

  reset() {
    this.phase = "setup";
    this.meetingName = "";
    this.localAudioLevel = 0;
    this.remoteAudioLevel = 0;
    this.elapsedSecs = 0;
    this.pipelineStep = "transcribing";
    this.pipelineProgress = 0;
    this.resultPath = "";
    this.errorMessage = "";
    this.createdIssues = [];
    this.localOggPath = "";
    this.remoteOggPath = "";
    this.pastedImages = [];
    this.liveSegments = [];
  }
}

export const appState = new AppState();

// Settings management — a separate reactive object for the settings UI
class SettingsState {
  groq_api_key: string = $state("");
  anthropic_api_key: string = $state("");
  together_ai_api_key: string = $state("");
  summarization_provider: string = $state("claude");
  together_ai_model: string = $state("meta-llama/Llama-3.3-70B-Instruct-Turbo");
  default_input_device: string = $state("");
  local_speaker_name: string = $state("Local");
  remote_device: string = $state("");
  remote_speaker_name: string = $state("Remote");
  working_folder: string = $state("");
  github_repo: string = $state("");
  meetings_subfolder: string = $state("Meetings");
  default_language: string = $state("fr");
  default_speakers: { name: string; organization: string }[] = $state([]);
  contexts: Context[] = $state([]);

  enable_summary: boolean = $state(true);
  enable_git_commit: boolean = $state(true);
  enable_github_issues: boolean = $state(true);

  loaded: boolean = $state(false);
  saving: boolean = $state(false);
  saveError: string = $state("");

  async load() {
    try {
      const s = await getSettings();
      this.groq_api_key = s.groq_api_key;
      this.anthropic_api_key = s.anthropic_api_key;
      this.together_ai_api_key = s.together_ai_api_key ?? "";
      this.summarization_provider = s.summarization_provider ?? "claude";
      this.together_ai_model = s.together_ai_model ?? "meta-llama/Llama-3.3-70B-Instruct-Turbo";
      this.default_input_device = s.default_input_device ?? "";
      this.local_speaker_name = s.local_speaker_name ?? "Local";
      this.remote_device = s.remote_device ?? "";
      this.remote_speaker_name = s.remote_speaker_name ?? "Remote";
      this.working_folder = s.working_folder;
      this.github_repo = s.github_repo;
      this.meetings_subfolder = s.meetings_subfolder || "Meetings";
      this.default_language = s.default_language || "fr";
      this.default_speakers = s.default_speakers ?? [];
      this.contexts = s.contexts ?? [];
      this.enable_summary = s.enable_summary ?? true;
      this.enable_git_commit = s.enable_git_commit ?? true;
      this.enable_github_issues = s.enable_github_issues ?? true;
      this.loaded = true;
    } catch (e) {
      console.error("Failed to load settings:", e);
    }
  }

  toSettings(): Settings {
    return {
      groq_api_key: this.groq_api_key,
      anthropic_api_key: this.anthropic_api_key,
      together_ai_api_key: this.together_ai_api_key,
      summarization_provider: this.summarization_provider,
      together_ai_model: this.together_ai_model,
      working_folder: this.working_folder,
      github_repo: this.github_repo,
      meetings_subfolder: this.meetings_subfolder,
      default_language: this.default_language,
      default_speakers: this.default_speakers,
      contexts: this.contexts,
      default_input_device: this.default_input_device,
      local_speaker_name: this.local_speaker_name,
      remote_device: this.remote_device,
      remote_speaker_name: this.remote_speaker_name,
      enable_summary: this.enable_summary,
      enable_git_commit: this.enable_git_commit,
      enable_github_issues: this.enable_github_issues,
    };
  }

  async save() {
    this.saving = true;
    this.saveError = "";
    try {
      await saveSettings(this.toSettings());
      // Sync appState so the main UI reflects the new settings immediately
      appState.applySettings(this.toSettings());
      appState.outputDir = await getDefaultOutputDir();
    } catch (e: any) {
      this.saveError = e?.toString() ?? "Unknown error";
    } finally {
      this.saving = false;
    }
  }
}

export const settingsState = new SettingsState();
