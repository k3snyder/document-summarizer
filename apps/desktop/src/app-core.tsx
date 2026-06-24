import * as React from "react";
import {
  DEFAULT_PIPELINE_CONFIG,
  DesktopJob,
  DesktopSettings,
  LogLevel,
  PdfImageDpi,
  PipelineConfig,
  ProviderAvailability,
  ProviderAvailabilityRole,
  ProviderVisibilitySettings,
  ReasoningEffort,
  SummarizerMode,
  SummarizerProvider,
  ThemePreference,
  VisibleVisionProvider,
  VisionMode,
} from "./types";

export type View = "process" | "history" | "logs" | "settings";
export type FileKind = "pdf" | "pptx" | "docx" | "text" | "mixed";
export type PipelineStepId =
  | "extraction"
  | "vision"
  | "chunking"
  | "summarization"
  | "review";
export type RuntimeStageId = "extraction" | "vision" | "summarization";
export type ProviderAvailabilityMap = Partial<
  Record<string, ProviderAvailability>
>;

export const DPI_OPTIONS: PdfImageDpi[] = [72, 144, 200, 300];
export const DEFAULT_PROVIDER_VISIBILITY: ProviderVisibilitySettings = {
  vision: {
    llama_cpp: false,
    ollama: false,
    openai: false,
    codex: true,
    claude: false,
    grok: false,
  },
  classifier: {
    llama_cpp: false,
    ollama: false,
    openai: false,
    codex: true,
    claude: false,
    grok: false,
  },
  summarizer: {
    llama_cpp: false,
    ollama: false,
    openai: false,
    codex: true,
    claude: false,
    grok: false,
  },
};
export const DEFAULT_LOGGING_SETTINGS = {
  enabled: true,
  level: "info" as LogLevel,
  retention_days: 14,
  max_file_mb: 50,
  capture_frontend: true,
  capture_dev_services: true,
  redact_secrets: true,
};
export const VISION_PROVIDER_OPTIONS: Array<{
  value: VisibleVisionProvider;
  label: string;
  description: string;
}> = [
  {
    value: "llama_cpp",
    label: "llama.cpp",
    description: "Locally hosted model",
  },
  { value: "ollama", label: "Ollama", description: "Locally hosted model" },
  { value: "openai", label: "OpenAI", description: "Cloud hosted model" },
  { value: "codex", label: "Codex CLI", description: "OpenAI Codex via CLI" },
  {
    value: "claude",
    label: "Claude CLI",
    description: "Anthropic Claude via CLI",
  },
  { value: "grok", label: "Grok CLI", description: "xAI Grok Build via CLI" },
];
export const SUMMARIZER_PROVIDER_OPTIONS: Array<{
  value: SummarizerProvider;
  label: string;
  description: string;
}> = [
  {
    value: "llama_cpp",
    label: "llama.cpp",
    description: "Locally hosted model",
  },
  { value: "ollama", label: "Ollama", description: "Locally hosted model" },
  { value: "openai", label: "OpenAI", description: "Cloud hosted model" },
  { value: "codex", label: "Codex CLI", description: "OpenAI Codex via CLI" },
  {
    value: "claude",
    label: "Claude CLI",
    description: "Anthropic Claude via CLI",
  },
  { value: "grok", label: "Grok CLI", description: "xAI Grok Build via CLI" },
];
export const SUMMARIZER_MODE_OPTIONS: Array<{
  value: SummarizerMode;
  label: string;
  description: string;
}> = [
  { value: "full", label: "Full", description: "Generate notes and topics" },
  {
    value: "topics-only",
    label: "Topics Only",
    description: "Only extract summary topics",
  },
  { value: "skip", label: "Skip", description: "Do not generate summaries" },
];
export const REASONING_EFFORT_OPTIONS: ReasoningEffort[] = [
  "low",
  "medium",
  "high",
  "xhigh",
  "max",
];

export function upsertJob(
  setJobs: React.Dispatch<React.SetStateAction<DesktopJob[]>>,
  job: DesktopJob,
) {
  setJobs((current) => {
    if (current.some((item) => item.job_id === job.job_id)) {
      return current.map((item) => (item.job_id === job.job_id ? job : item));
    }
    return [job, ...current];
  });
}

export function errorMessage(error: unknown): string {
  if (typeof error === "string") return error;
  if (error && typeof error === "object" && "message" in error) {
    return String((error as { message?: unknown }).message);
  }
  return "Unexpected error.";
}

export function useElapsedMilliseconds(
  startedAt?: string | null,
  completedDurationMs?: number | null,
): number {
  const [now, setNow] = React.useState(() => Date.now());

  React.useEffect(() => {
    if (!startedAt || completedDurationMs != null) return;
    setNow(Date.now());
    const interval = window.setInterval(() => setNow(Date.now()), 1000);
    return () => window.clearInterval(interval);
  }, [completedDurationMs, startedAt]);

  if (completedDurationMs != null) return completedDurationMs;
  if (!startedAt) return 0;
  const started = Date.parse(startedAt);
  if (Number.isNaN(started)) return 0;
  return Math.max(0, now - started);
}

export function applyTheme(theme: ThemePreference) {
  const resolved =
    theme === "system"
      ? window.matchMedia("(prefers-color-scheme: dark)").matches
        ? "dark"
        : "light"
      : theme;
  document.documentElement.dataset.theme = resolved;
}

export function basename(path: string): string {
  return path.split(/[\\/]/).pop() ?? path;
}

export function displaySettingsPath(path: string): string {
  if (!path) return "~/.summarizer/settings.json";
  return path.endsWith("/.summarizer/settings.json")
    ? "~/.summarizer/settings.json"
    : path;
}

export function stripExtension(fileName: string): string {
  return fileName.replace(/\.[^/.]+$/, "");
}

export function visibleVisionProviderOptions(
  visibility: ProviderVisibilitySettings["vision"],
) {
  const options = VISION_PROVIDER_OPTIONS.filter(
    (option) => visibility[option.value],
  );
  return options.length
    ? options
    : VISION_PROVIDER_OPTIONS.filter((option) => option.value === "codex");
}

export function visibleSummarizerProviderOptions(
  visibility: ProviderVisibilitySettings["summarizer"],
) {
  const options = SUMMARIZER_PROVIDER_OPTIONS.filter(
    (option) => visibility[option.value],
  );
  return options.length
    ? options
    : SUMMARIZER_PROVIDER_OPTIONS.filter((option) => option.value === "codex");
}

export function providerAvailabilityKey(
  role: ProviderAvailabilityRole,
  provider: string,
): string {
  return `${role}:${provider}`;
}

export function normalizeDesktopSettings(
  settings: DesktopSettings,
): DesktopSettings {
  const pipelineDefaults =
    settings.pipeline_defaults ?? DEFAULT_PIPELINE_CONFIG;
  return {
    ...settings,
    pipeline_defaults: {
      ...DEFAULT_PIPELINE_CONFIG,
      ...pipelineDefaults,
      pdf_image_dpi: normalizePdfImageDpi(
        (pipelineDefaults as { pdf_image_dpi?: unknown }).pdf_image_dpi,
      ),
    },
    provider_visibility:
      settings.provider_visibility ?? DEFAULT_PROVIDER_VISIBILITY,
    logging: {
      ...DEFAULT_LOGGING_SETTINGS,
      ...(settings.logging ?? {}),
    },
  };
}

export function normalizePdfImageDpi(value: unknown): PdfImageDpi {
  const numeric = typeof value === "string" ? Number(value) : value;
  return DPI_OPTIONS.includes(numeric as PdfImageDpi)
    ? (numeric as PdfImageDpi)
    : DEFAULT_PIPELINE_CONFIG.pdf_image_dpi;
}

export function firstVisibleVisionProvider(
  visibility: ProviderVisibilitySettings["vision"],
): VisibleVisionProvider {
  return visibleVisionProviderOptions(visibility)[0]?.value ?? "codex";
}

export function firstVisibleSummarizerProvider(
  visibility: ProviderVisibilitySettings["summarizer"],
): SummarizerProvider {
  return visibleSummarizerProviderOptions(visibility)[0]?.value ?? "codex";
}

export function normalizedConfigForProviderVisibility(
  config: PipelineConfig,
  providerVisibility: ProviderVisibilitySettings,
): PipelineConfig | null {
  const nextVision = firstVisibleVisionProvider(providerVisibility.vision);
  const nextClassifier = firstVisibleVisionProvider(
    providerVisibility.classifier,
  );
  const nextSummarizer = firstVisibleSummarizerProvider(
    providerVisibility.summarizer,
  );
  let nextConfig: PipelineConfig | null = null;
  const patch = <K extends keyof PipelineConfig>(
    key: K,
    value: PipelineConfig[K],
  ) => {
    nextConfig = { ...(nextConfig ?? config), [key]: value };
  };

  if (
    config.vision_mode !== "none" &&
    !isVisibleVisionProvider(providerVisibility.vision, config.vision_mode)
  ) {
    patch("vision_mode", nextVision);
  }
  if (
    config.vision_classifier_mode &&
    !isVisibleVisionProvider(
      providerVisibility.classifier,
      config.vision_classifier_mode,
    )
  ) {
    patch("vision_classifier_mode", nextClassifier);
  }
  if (
    config.vision_extractor_mode &&
    !isVisibleVisionProvider(
      providerVisibility.vision,
      config.vision_extractor_mode,
    )
  ) {
    patch("vision_extractor_mode", nextVision);
  }
  if (!providerVisibility.summarizer[config.summarizer_provider]) {
    patch("summarizer_provider", nextSummarizer);
  }

  return nextConfig;
}

export function isVisibleVisionProvider(
  visibility: ProviderVisibilitySettings["vision"],
  value: VisionMode,
): value is VisibleVisionProvider {
  return (
    value !== "none" &&
    value !== "deepseek" &&
    value !== "gemini" &&
    visibility[value]
  );
}

export function normalizedVisionValue(
  value: VisionMode,
  options: Array<{
    value: VisibleVisionProvider;
    label: string;
    description: string;
  }>,
): VisibleVisionProvider {
  return options.some((option) => option.value === value) &&
    value !== "none" &&
    value !== "deepseek" &&
    value !== "gemini"
    ? value
    : (options[0]?.value ?? "codex");
}

export function normalizedSummarizerValue(
  value: SummarizerProvider,
  options: Array<{
    value: SummarizerProvider;
    label: string;
    description: string;
  }>,
): SummarizerProvider {
  return options.some((option) => option.value === value)
    ? value
    : (options[0]?.value ?? "codex");
}

export function enabledCount(values: Record<string, boolean>): number {
  return Object.values(values).filter(Boolean).length;
}

export function fileExtension(path: string): string {
  return (
    path
      .split(/[\\/]/)
      .pop()
      ?.toLowerCase()
      .match(/\.[^.]+$/)?.[0] ?? ""
  );
}

export function isSupportedInputPath(path: string): boolean {
  return [".pdf", ".pptx", ".docx", ".txt", ".md", ".markdown"].includes(
    fileExtension(path),
  );
}

export function getFileKind(path: string): FileKind {
  const extension = fileExtension(path);
  if (extension === ".pptx") return "pptx";
  if (extension === ".docx") return "docx";
  if (extension === ".txt" || extension === ".md" || extension === ".markdown")
    return "text";
  return "pdf";
}

export function getBatchFileKind(paths: string[]): FileKind {
  const kinds = Array.from(new Set(paths.map(getFileKind)));
  return kinds.length === 1 ? kinds[0] : "mixed";
}

export function buildPipelineSteps(
  fileKind: FileKind,
  extractOnly: boolean,
): Array<{ id: PipelineStepId; title: string; description: string }> {
  if (fileKind === "text") {
    return [
      {
        id: "chunking",
        title: "Chunking",
        description: "Set how text files are split before processing.",
      },
      {
        id: "summarization",
        title: "Summarization",
        description: "Choose the summary provider and output style.",
      },
      {
        id: "review",
        title: "Review",
        description: "Review and submit the configuration.",
      },
    ];
  }
  const extractionDescription =
    fileKind === "pptx"
      ? "Configure PowerPoint parsing options."
      : fileKind === "docx"
        ? "Configure Word parsing options."
        : fileKind === "mixed"
          ? "Configure shared document parsing options."
          : "Configure PDF parsing options.";
  if (extractOnly) {
    return [
      {
        id: "extraction",
        title: "Extraction",
        description: extractionDescription,
      },
      {
        id: "review",
        title: "Review",
        description: "Review and submit the configuration.",
      },
    ];
  }
  if (fileKind === "mixed") {
    return [
      {
        id: "extraction",
        title: "Extraction",
        description: extractionDescription,
      },
      {
        id: "vision",
        title: "Vision",
        description: "Optionally analyze visual content in rendered pages.",
      },
      {
        id: "chunking",
        title: "Chunking",
        description: "Set how text files are split before processing.",
      },
      {
        id: "summarization",
        title: "Summarization",
        description: "Choose the summary provider and output style.",
      },
      {
        id: "review",
        title: "Review",
        description: "Review and submit the configuration.",
      },
    ];
  }
  return [
    {
      id: "extraction",
      title: "Extraction",
      description: extractionDescription,
    },
    {
      id: "vision",
      title: "Vision",
      description: "Optionally analyze visual content in rendered pages.",
    },
    {
      id: "summarization",
      title: "Summarization",
      description: "Choose the summary provider and output style.",
    },
    {
      id: "review",
      title: "Review",
      description: "Review and submit the configuration.",
    },
  ];
}

export function buildProcessingStages(
  config: PipelineConfig,
): Array<{ id: RuntimeStageId; label: string }> {
  const stages: Array<{ id: RuntimeStageId; label: string }> = [
    { id: "extraction", label: "Extraction" },
  ];
  if (!config.extract_only && config.vision_mode !== "none") {
    stages.push({ id: "vision", label: "Vision" });
  }
  if (
    !config.extract_only &&
    config.run_summarization &&
    config.summarizer_mode !== "skip"
  ) {
    stages.push({ id: "summarization", label: "Summarization" });
  }
  return stages;
}

export function progressUnitLabel(fileName: string): string {
  const kind = getFileKind(fileName);
  if (kind === "pptx") return "Slide";
  if (kind === "text") return "Chunk";
  if (kind === "docx") return "Chunk";
  return "Page";
}

export function orderQueuedJobs(jobs: DesktopJob[]): DesktopJob[] {
  return jobs
    .filter((job) => job.status === "queued")
    .sort(
      (left, right) =>
        sortableTimestamp(left.queued_at ?? left.created_at) -
        sortableTimestamp(right.queued_at ?? right.created_at),
    );
}

export function queueJobSubtitle(job: DesktopJob): string {
  if (job.status === "queued")
    return `Queued ${formatDateTime(job.queued_at ?? job.created_at)}`;
  if (job.status === "processing") {
    const suffix = job.error ? ` / ${job.error}` : "";
    return `Processing${suffix}`;
  }
  return `${labelize(job.status)} / ${formatDuration(job.duration_ms)}`;
}

export function sortableTimestamp(value?: string | null): number {
  if (!value) return 0;
  const timestamp = Date.parse(value);
  return Number.isNaN(timestamp) ? 0 : timestamp;
}

export function formatDateTime(value?: string | null): string {
  const timestamp = sortableTimestamp(value);
  if (!timestamp) return "recently";
  return new Intl.DateTimeFormat(undefined, {
    month: "short",
    day: "numeric",
    hour: "numeric",
    minute: "2-digit",
  }).format(timestamp);
}

export function formatLogTimestamp(value: string): string {
  const timestamp = sortableTimestamp(value);
  if (!timestamp) return "";
  return new Intl.DateTimeFormat(undefined, {
    hour: "numeric",
    minute: "2-digit",
    second: "2-digit",
  }).format(timestamp);
}

export function formatBytes(value: number): string {
  if (value < 1024) return `${value} B`;
  if (value < 1_048_576) return `${(value / 1024).toFixed(1)} KB`;
  return `${(value / 1_048_576).toFixed(1)} MB`;
}

export function safeJson(value: unknown): string {
  try {
    return JSON.stringify(value, null, 2);
  } catch {
    return String(value);
  }
}

export function clampProgress(value: number): number {
  if (!Number.isFinite(value)) return 1;
  return Math.max(1, Math.min(100, Math.round(value)));
}

export function dpiLabel(
  dpi: PdfImageDpi | string | number | null | undefined,
): string {
  const normalized = normalizePdfImageDpi(dpi);
  if (normalized === 72) return "72 DPI (Fast)";
  if (normalized === 144) return "144 DPI (Balanced)";
  if (normalized === 200) return "200 DPI (Default)";
  return "300 DPI (High Quality)";
}

export function visionLabel(mode: VisionMode): string {
  if (mode === "none") return "Disabled";
  return (
    VISION_PROVIDER_OPTIONS.find((option) => option.value === mode)?.label ??
    labelize(mode)
  );
}

export function summarizerProviderLabel(provider: SummarizerProvider): string {
  return (
    SUMMARIZER_PROVIDER_OPTIONS.find((option) => option.value === provider)
      ?.label ?? labelize(provider)
  );
}

export function summarizerModeLabel(mode: SummarizerMode): string {
  return (
    SUMMARIZER_MODE_OPTIONS.find((option) => option.value === mode)?.label ??
    labelize(mode)
  );
}

export function formatMetricProvider(provider?: string | null): string {
  if (!provider || provider === "none" || provider === "skip")
    return "Disabled";
  if (provider === "llama_cpp") return "llama.cpp";
  if (provider === "openai") return "OpenAI";
  if (provider === "codex") return "Codex CLI";
  if (provider === "claude") return "Claude CLI";
  if (provider === "grok") return "Grok CLI";
  return labelize(provider);
}

export function formatDuration(durationMs?: number | null): string {
  if (durationMs == null) return "not finished";
  if (durationMs < 1000) return `${durationMs} ms`;
  if (durationMs >= 60_000) return formatElapsedDuration(durationMs);
  return `${(durationMs / 1000).toFixed(1)} s`;
}

export function formatElapsedDuration(durationMs: number): string {
  const totalSeconds = Math.floor(durationMs / 1000);
  const seconds = totalSeconds % 60;
  const totalMinutes = Math.floor(totalSeconds / 60);
  const minutes = totalMinutes % 60;
  const hours = Math.floor(totalMinutes / 60);
  if (hours > 0)
    return `${hours}h ${String(minutes).padStart(2, "0")}m ${String(seconds).padStart(2, "0")}s`;
  if (minutes > 0) return `${minutes}m ${String(seconds).padStart(2, "0")}s`;
  return `${seconds}s`;
}

export function labelize(value: string): string {
  return value.replace(/_/g, " ").replace(/-/g, " ");
}

export function titleize(value: string): string {
  return labelize(value).replace(/\b\w/g, (letter) => letter.toUpperCase());
}

export function imageSrc(value: string): string {
  return value.startsWith("data:") ? value : `data:image/png;base64,${value}`;
}

export function notesText(notes?: string[] | null): string | null {
  return notes?.length ? notes.map((note) => `- ${note}`).join("\n") : null;
}
