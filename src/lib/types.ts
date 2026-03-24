export interface Speaker {
  name: string;
  organization: string;
  enabled: boolean;
}

export interface Context {
  id: string;
  label: string;
  content: string;
}

export interface Settings {
  groq_api_key: string;
  anthropic_api_key: string;
  working_folder: string;
  github_repo: string;
  meetings_subfolder: string;
  default_language: string;
  default_speakers: { name: string; organization: string }[];
  contexts: Context[];
  default_input_device: string;
  together_ai_api_key: string;
  summarization_provider: string;
  together_ai_model: string;
  enable_summary: boolean;
  enable_git_commit: boolean;
  enable_github_issues: boolean;
}

export interface PipelineConfig {
  context: string;
  context_content: string;
  meeting_name: string;
  speakers: { name: string; organization: string }[];
  language_code: string;
  language_name: string;
  github_repo: string;
  output_dir: string;
  working_folder: string;
  ogg_path: string;
  wav_path: string;
}

export interface CreatedIssue {
  number: number;
  title: string;
  url: string;
}

export interface PipelineResult {
  notes_path: string;
  created_issues: CreatedIssue[];
}

export type AppPhase = "setup" | "recording" | "processing" | "result" | "error";

export type PipelineStep =
  | "transcribing"
  | "diarizing"
  | "merging"
  | "summarizing"
  | "exporting"
  | "committing"
  | "creating_issues"
  | "done";
