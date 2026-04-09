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
  local_speaker_name: string;
  remote_device: string;
  remote_speaker_name: string;
  together_ai_api_key: string;
  summarization_provider: string;
  together_ai_model: string;
  enable_summary: boolean;
  enable_git_commit: boolean;
  enable_github_issues: boolean;
}

export interface ImageAnnotation {
  path: string;
  timecode_secs: number;
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
  local_ogg_path: string;
  local_speaker_name: string;
  remote_ogg_path: string;
  remote_speaker_name: string;
  image_annotations: ImageAnnotation[];
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
  | "reusing_live"
  | "diarizing"
  | "merging"
  | "summarizing"
  | "exporting"
  | "committing"
  | "creating_issues"
  | "done";

export interface LiveSegment {
  speaker: string;
  text: string;
  start: number;
  end: number;
}
