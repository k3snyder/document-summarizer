export type PdfImageDpi = 72 | 144 | 200 | 300;
export type VisionMode =
  | "none"
  | "deepseek"
  | "gemini"
  | "grok"
  | "openai"
  | "ollama"
  | "llama_cpp"
  | "codex"
  | "claude";
export type CliProvider = "codex" | "claude" | "grok";
export type SummarizerMode = "full" | "topics-only" | "skip";
export type SummarizerProvider =
  | "ollama"
  | "llama_cpp"
  | "openai"
  | "codex"
  | "claude"
  | "grok";
export type ThemePreference = "system" | "light" | "dark";
export type VisibleVisionProvider = Exclude<
  VisionMode,
  "none" | "deepseek" | "gemini"
>;
export type ReasoningEffort = "low" | "medium" | "high" | "xhigh" | "max";

export const RUST_PDF_IMAGE_DPI_SERDE_NAMES = [
  "72",
  "144",
  "200",
  "300",
] as const;
export const RUST_VISION_MODE_SERDE_NAMES = [
  "none",
  "deepseek",
  "gemini",
  "openai",
  "ollama",
  "llama_cpp",
  "codex",
  "claude",
  "grok",
] as const;
export const RUST_SUMMARIZER_PROVIDER_SERDE_NAMES = [
  "ollama",
  "llama_cpp",
  "openai",
  "codex",
  "claude",
  "grok",
] as const;
export const RUST_CLI_PROVIDER_SERDE_NAMES = [
  "codex",
  "claude",
  "grok",
] as const;

type ExactUnion<
  Actual extends string | number,
  Expected extends string | number,
> =
  Exclude<Actual, Expected> extends never
    ? Exclude<Expected, Actual> extends never
      ? true
      : never
    : never;
export const PDF_IMAGE_DPI_SERDE_CONTRACT: ExactUnion<
  (typeof RUST_PDF_IMAGE_DPI_SERDE_NAMES)[number],
  `${PdfImageDpi}`
> = true;
export const VISION_MODE_SERDE_CONTRACT: ExactUnion<
  (typeof RUST_VISION_MODE_SERDE_NAMES)[number],
  VisionMode
> = true;
export const SUMMARIZER_PROVIDER_SERDE_CONTRACT: ExactUnion<
  (typeof RUST_SUMMARIZER_PROVIDER_SERDE_NAMES)[number],
  SummarizerProvider
> = true;
export const CLI_PROVIDER_SERDE_CONTRACT: ExactUnion<
  (typeof RUST_CLI_PROVIDER_SERDE_NAMES)[number],
  CliProvider
> = true;

export interface ProviderVisibilitySettings {
  vision: Record<VisibleVisionProvider, boolean>;
  classifier: Record<VisibleVisionProvider, boolean>;
  summarizer: Record<SummarizerProvider, boolean>;
}

export interface PipelineConfig {
  run_extraction: boolean;
  extract_only: boolean;
  skip_tables: boolean;
  skip_images: boolean;
  skip_pptx_tables: boolean;
  text_only: boolean;
  pdf_image_dpi: PdfImageDpi;
  vision_mode: VisionMode;
  vision_classifier_mode?: VisionMode;
  vision_extractor_mode?: VisionMode;
  vision_cli_provider?: CliProvider;
  vision_skip_classification: boolean;
  chunk_size: number;
  chunk_overlap: number;
  run_summarization: boolean;
  summarizer_mode: SummarizerMode;
  summarizer_provider: SummarizerProvider;
  summarizer_detailed_extraction: boolean;
  summarizer_insight_mode: boolean;
  summarizer_cli_provider?: CliProvider;
  max_tokens_per_page: number;
  max_seconds_per_page: number;
  keep_base64_images: boolean;
}

export const DEFAULT_PIPELINE_CONFIG: PipelineConfig = {
  run_extraction: true,
  extract_only: false,
  skip_tables: false,
  skip_images: false,
  skip_pptx_tables: false,
  text_only: false,
  pdf_image_dpi: 200,
  vision_mode: "codex",
  vision_skip_classification: false,
  chunk_size: 3000,
  chunk_overlap: 80,
  run_summarization: true,
  summarizer_mode: "full",
  summarizer_provider: "codex",
  summarizer_detailed_extraction: false,
  summarizer_insight_mode: false,
  max_tokens_per_page: 100000,
  max_seconds_per_page: 300,
  keep_base64_images: false,
};

export interface DocumentMetadata {
  document_id: string;
  filename: string;
  total_pages: number;
  metadata: Record<string, unknown>;
}

export interface ExtractedImage {
  id: string;
  relationship_id?: string | null;
  content_type?: string | null;
  filename?: string | null;
  alt_text?: string | null;
  base64?: string | null;
}

export interface TableCell {
  text: string;
  row_span?: number | null;
  col_span?: number | null;
  metadata?: Record<string, unknown>;
}

export interface PageOutput {
  chunk_id: string;
  doc_title: string;
  page_number?: number | null;
  text: string;
  tables: string[][][];
  extraction_warnings?: string[];
  html?: string | null;
  embedded_images?: ExtractedImage[];
  image_base64?: string | null;
  image_text?: string | null;
  image_classifier?: boolean | null;
  image_text_1?: string | null;
  image_text_2?: string | null;
  image_text_3?: string | null;
  summary_notes?: string[] | null;
  summary_topics?: string[] | null;
  summary_relevancy?: number | null;
  summary_quality_validated?: boolean | null;
  summary_notes_1?: string[] | null;
  summary_notes_2?: string[] | null;
  summary_notes_3?: string[] | null;
  summary_budget_exhausted?: string | null;
}

export interface StageMetrics {
  duration_ms: number;
  pages_processed: number;
  tokens: number;
  pages_with_images?: number | null;
  classified_count?: number | null;
  extracted_count?: number | null;
  avg_relevancy?: number | null;
  total_attempts?: number | null;
}

export interface PipelineMetrics {
  total_duration_ms: number;
  total_tokens: number;
  stages: {
    extraction: StageMetrics;
    vision: StageMetrics;
    summarization: StageMetrics;
  };
  config: {
    vision_mode: string | null;
    vision_classifier_provider?: string | null;
    vision_extractor_provider?: string | null;
    summarizer_provider: string | null;
    summarizer_mode: string | null;
  };
}

export interface DocumentOutput {
  document: DocumentMetadata;
  pages: PageOutput[];
  metrics?: PipelineMetrics | null;
}

export type DesktopJobStatus =
  | "queued"
  | "processing"
  | "completed"
  | "failed"
  | "canceled";

export interface DesktopJob {
  job_id: string;
  status: DesktopJobStatus;
  file_path: string;
  file_name: string;
  created_at: string;
  queued_at?: string | null;
  started_at?: string | null;
  completed_at?: string | null;
  duration_ms?: number | null;
  error?: string | null;
  config?: PipelineConfig | null;
  output?: DocumentOutput | null;
}

export type PipelineProgressStage =
  | "extraction"
  | "vision"
  | "summarization"
  | "completed";

export interface DesktopJobProgress {
  job_id: string;
  file_name: string;
  stage: PipelineProgressStage;
  stage_label: string;
  stage_index: number;
  total_stages: number;
  page_number?: number | null;
  total_pages?: number | null;
  progress: number;
  message: string;
}

export interface OpenAiSettings {
  base_url: string;
  api_key: string;
  model: string;
  model_2: string;
  model_3: string;
  vision_model: string;
}

export interface LlamaCppSettings {
  base_url: string;
  vision_base_url: string;
  api_key: string;
  model: string;
  model_2: string;
  model_3: string;
  vision_model: string;
}

export interface OllamaSettings {
  openai_base_url: string;
  api_key: string;
  model: string;
  model_2: string;
  model_3: string;
  vision_model: string;
}

export interface CliSettings {
  executable: string;
  args: string;
  reasoning_effort: ReasoningEffort;
  timeout_seconds: number;
}

export interface ProviderSettings {
  openai: OpenAiSettings;
  llama_cpp: LlamaCppSettings;
  ollama: OllamaSettings;
  codex: CliSettings;
  claude: CliSettings;
  grok: CliSettings;
}

export interface DesktopSettings {
  appearance: {
    theme: ThemePreference;
  };
  providers: ProviderSettings;
  pipeline_defaults: PipelineConfig;
  provider_visibility: ProviderVisibilitySettings;
  logging: LoggingSettings;
}

export type LogLevel = "trace" | "debug" | "info" | "warn" | "error";
export type RuntimeSource = "desktop" | "frontend" | "dev_service";

export interface LoggingSettings {
  enabled: boolean;
  level: LogLevel;
  retention_days: number;
  max_file_mb: number;
  capture_frontend: boolean;
  capture_dev_services: boolean;
  redact_secrets: boolean;
}

export interface LogEvent {
  seq: number;
  timestamp: string;
  source: RuntimeSource;
  level: LogLevel;
  message: string;
  target: string;
  file?: string | null;
  line?: number | null;
  jobId?: string | null;
  stage?: string | null;
  fields: Record<string, unknown> | unknown[];
}

export interface LogFileMeta {
  name: string;
  path: string;
  sizeBytes: number;
  modifiedAt?: string | null;
}

export interface LogPathInfo {
  logDir: string;
  activeFile: string;
}

export type ProviderReadinessStatus = "ready" | "offline";
export type ProviderAvailabilityRole = "vision" | "summarizer";

export interface ProviderReadiness {
  status: ProviderReadinessStatus;
  provider?: string | null;
  message: string;
}

export interface ProviderAvailability {
  role: ProviderAvailabilityRole;
  provider: string;
  label: string;
  status: ProviderReadinessStatus;
  message: string;
}

export interface CommandError {
  code?: string;
  message?: string;
}
