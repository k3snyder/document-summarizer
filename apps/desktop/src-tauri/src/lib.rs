use chrono::{DateTime, Duration as ChronoDuration, Utc};
use serde::{Deserialize, Serialize};
use std::{
    collections::HashSet,
    env, fs, io,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::{Arc, RwLock},
    time::Duration,
};
use summarizer_cli_util::{
    cli_search_path_with_extra_dirs, resolve_cli_executable_with_extra_dirs, resolve_soffice,
    suppress_command_window,
};
use summarizer_pipeline::{
    configure_pdfium_library_path, CliRuntimeConfig, GeminiProviderConfig, HttpProviderConfig,
    LlamaCppProviderConfig, OllamaProviderConfig, Pipeline, PipelineProgress,
    PipelineProviderConfig,
};
use summarizer_types::{
    CliProvider, DocumentOutput, PipelineConfig, PipelineError, SummarizerMode, SummarizerProvider,
    VisionMode,
};
use tauri::{AppHandle, Emitter, Manager, State};
use tokio::sync::Mutex;
use uuid::Uuid;

mod logs;

type AppResult<T> = Result<T, AppError>;

#[derive(Debug, Clone, Serialize)]
struct AppError {
    code: &'static str,
    message: String,
}

impl AppError {
    fn new(code: &'static str, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }

    fn storage(message: impl Into<String>) -> Self {
        Self::new("storage", message)
    }
}

impl From<PipelineError> for AppError {
    fn from(value: PipelineError) -> Self {
        Self::new("pipeline", value.to_string())
    }
}

#[derive(Clone)]
struct DesktopState {
    jobs: Arc<Mutex<JobStore>>,
    settings: Arc<RwLock<DesktopSettings>>,
}

impl DesktopState {
    fn new(settings: DesktopSettings, jobs: Vec<DesktopJob>) -> Self {
        Self {
            jobs: Arc::new(Mutex::new(JobStore::from_jobs(jobs))),
            settings: Arc::new(RwLock::new(settings)),
        }
    }
}

#[derive(Debug, Default)]
struct JobStore {
    active_job_id: Option<String>,
    canceled: HashSet<String>,
    jobs: Vec<DesktopJob>,
    worker_running: bool,
}

impl JobStore {
    fn from_jobs(jobs: Vec<DesktopJob>) -> Self {
        Self {
            jobs,
            ..Default::default()
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum DesktopJobStatus {
    Queued,
    Processing,
    Completed,
    Failed,
    Canceled,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct DesktopJob {
    job_id: String,
    status: DesktopJobStatus,
    file_path: String,
    file_name: String,
    created_at: DateTime<Utc>,
    queued_at: Option<DateTime<Utc>>,
    started_at: Option<DateTime<Utc>>,
    completed_at: Option<DateTime<Utc>>,
    duration_ms: Option<u64>,
    error: Option<String>,
    config: Option<PipelineConfig>,
    output: Option<DocumentOutput>,
}

#[derive(Debug, Clone)]
struct QueueClaim {
    job: DesktopJob,
    path: PathBuf,
    config: Option<PipelineConfig>,
    started_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum JobCancelEvent {
    Updated,
    Canceled,
}

impl JobCancelEvent {
    fn as_str(self) -> &'static str {
        match self {
            Self::Updated => "job:updated",
            Self::Canceled => "job:canceled",
        }
    }
}

#[derive(Debug, Clone, Serialize)]
struct DesktopJobProgress {
    job_id: String,
    file_name: String,
    stage: String,
    stage_label: String,
    stage_index: usize,
    total_stages: usize,
    page_number: Option<usize>,
    total_pages: Option<usize>,
    progress: u8,
    message: String,
}

impl DesktopJobProgress {
    fn from_pipeline(job_id: &str, file_name: &str, progress: PipelineProgress) -> Self {
        Self {
            job_id: job_id.to_string(),
            file_name: file_name.to_string(),
            stage: progress.stage.as_str().to_string(),
            stage_label: progress.stage.label().to_string(),
            stage_index: progress.stage_index,
            total_stages: progress.total_stages,
            page_number: progress.page_number,
            total_pages: progress.total_pages,
            progress: progress.progress,
            message: progress.message,
        }
    }

    fn completed(job_id: &str, file_name: &str) -> Self {
        Self {
            job_id: job_id.to_string(),
            file_name: file_name.to_string(),
            stage: "completed".to_string(),
            stage_label: "Complete".to_string(),
            stage_index: 0,
            total_stages: 1,
            page_number: None,
            total_pages: None,
            progress: 100,
            message: "Processing complete.".to_string(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum ThemePreference {
    System,
    Light,
    Dark,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct AppearanceSettings {
    theme: ThemePreference,
}

impl Default for AppearanceSettings {
    fn default() -> Self {
        Self {
            theme: ThemePreference::Light,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct OpenAiSettings {
    base_url: String,
    api_key: String,
    model: String,
    model_2: String,
    model_3: String,
    vision_model: String,
}

impl Default for OpenAiSettings {
    fn default() -> Self {
        Self {
            base_url: "https://api.openai.com/v1".to_string(),
            api_key: String::new(),
            model: "gpt-4.1-mini".to_string(),
            model_2: "gpt-4.1-mini".to_string(),
            model_3: "gpt-4.1-mini".to_string(),
            vision_model: "gpt-4.1-mini".to_string(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct LlamaCppSettings {
    base_url: String,
    vision_base_url: String,
    api_key: String,
    model: String,
    model_2: String,
    model_3: String,
    vision_model: String,
}

impl Default for LlamaCppSettings {
    fn default() -> Self {
        Self {
            base_url: "http://localhost:11440/v1".to_string(),
            vision_base_url: "http://localhost:11439/v1".to_string(),
            api_key: String::new(),
            model: "model.gguf".to_string(),
            model_2: "model.gguf".to_string(),
            model_3: "model.gguf".to_string(),
            vision_model: "model.gguf".to_string(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct OllamaSettings {
    openai_base_url: String,
    api_key: String,
    model: String,
    model_2: String,
    model_3: String,
    vision_model: String,
}

impl Default for OllamaSettings {
    fn default() -> Self {
        Self {
            openai_base_url: "http://localhost:11434/v1".to_string(),
            api_key: String::new(),
            model: "llama3.2".to_string(),
            model_2: "llama3.2".to_string(),
            model_3: "llama3.2".to_string(),
            vision_model: "llava".to_string(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct CliSettings {
    executable: String,
    args: String,
    #[serde(default = "default_reasoning_effort")]
    reasoning_effort: String,
    timeout_seconds: u64,
}

impl CliSettings {
    fn new(executable: &str) -> Self {
        Self {
            executable: executable.to_string(),
            args: String::new(),
            reasoning_effort: default_reasoning_effort(),
            timeout_seconds: 600,
        }
    }
}

fn default_reasoning_effort() -> String {
    "medium".to_string()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ProviderSettings {
    openai: OpenAiSettings,
    llama_cpp: LlamaCppSettings,
    ollama: OllamaSettings,
    #[serde(default = "default_codex_settings")]
    codex: CliSettings,
    #[serde(default = "default_claude_settings")]
    claude: CliSettings,
    #[serde(default = "default_grok_settings")]
    grok: CliSettings,
}

impl Default for ProviderSettings {
    fn default() -> Self {
        Self {
            openai: OpenAiSettings::default(),
            llama_cpp: LlamaCppSettings::default(),
            ollama: OllamaSettings::default(),
            codex: default_codex_settings(),
            claude: default_claude_settings(),
            grok: default_grok_settings(),
        }
    }
}

fn default_codex_settings() -> CliSettings {
    CliSettings::new("codex")
}

fn default_claude_settings() -> CliSettings {
    CliSettings::new("claude")
}

fn default_grok_settings() -> CliSettings {
    CliSettings::new("grok")
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct VisionProviderVisibilitySettings {
    llama_cpp: bool,
    ollama: bool,
    openai: bool,
    #[serde(default)]
    codex: bool,
    #[serde(default)]
    claude: bool,
    #[serde(default)]
    grok: bool,
}

impl Default for VisionProviderVisibilitySettings {
    fn default() -> Self {
        Self {
            llama_cpp: false,
            ollama: false,
            openai: false,
            codex: true,
            claude: false,
            grok: false,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct SummarizerProviderVisibilitySettings {
    llama_cpp: bool,
    ollama: bool,
    openai: bool,
    #[serde(default)]
    codex: bool,
    #[serde(default)]
    claude: bool,
    #[serde(default)]
    grok: bool,
}

impl Default for SummarizerProviderVisibilitySettings {
    fn default() -> Self {
        Self {
            llama_cpp: false,
            ollama: false,
            openai: false,
            codex: true,
            claude: false,
            grok: false,
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct ProviderVisibilitySettings {
    vision: VisionProviderVisibilitySettings,
    classifier: VisionProviderVisibilitySettings,
    summarizer: SummarizerProviderVisibilitySettings,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct LoggingSettings {
    #[serde(default = "default_logging_enabled")]
    enabled: bool,
    #[serde(default = "default_log_level")]
    level: String,
    #[serde(default = "default_log_retention_days")]
    retention_days: u16,
    #[serde(default = "default_log_max_file_mb")]
    max_file_mb: u16,
    #[serde(default = "default_capture_frontend_logs")]
    capture_frontend: bool,
    #[serde(default = "default_capture_dev_service_logs")]
    capture_dev_services: bool,
    #[serde(default = "default_redact_log_secrets")]
    redact_secrets: bool,
}

impl Default for LoggingSettings {
    fn default() -> Self {
        Self {
            enabled: default_logging_enabled(),
            level: default_log_level(),
            retention_days: default_log_retention_days(),
            max_file_mb: default_log_max_file_mb(),
            capture_frontend: default_capture_frontend_logs(),
            capture_dev_services: default_capture_dev_service_logs(),
            redact_secrets: default_redact_log_secrets(),
        }
    }
}

fn default_logging_enabled() -> bool {
    true
}

fn default_log_level() -> String {
    "info".to_string()
}

fn default_log_retention_days() -> u16 {
    14
}

fn default_log_max_file_mb() -> u16 {
    50
}

fn default_capture_frontend_logs() -> bool {
    true
}

fn default_capture_dev_service_logs() -> bool {
    true
}

fn default_redact_log_secrets() -> bool {
    true
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct DesktopSettings {
    appearance: AppearanceSettings,
    providers: ProviderSettings,
    #[serde(default = "desktop_default_pipeline_config")]
    pipeline_defaults: PipelineConfig,
    #[serde(default)]
    provider_visibility: ProviderVisibilitySettings,
    #[serde(default)]
    logging: LoggingSettings,
}

impl Default for DesktopSettings {
    fn default() -> Self {
        Self {
            appearance: AppearanceSettings::default(),
            providers: ProviderSettings::default(),
            pipeline_defaults: desktop_default_pipeline_config(),
            provider_visibility: ProviderVisibilitySettings::default(),
            logging: LoggingSettings::default(),
        }
    }
}

fn desktop_default_pipeline_config() -> PipelineConfig {
    PipelineConfig {
        vision_mode: VisionMode::Codex,
        summarizer_provider: SummarizerProvider::Codex,
        ..PipelineConfig::default()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
enum ProviderReadinessStatus {
    Ready,
    Offline,
}

#[derive(Debug, Clone, Serialize)]
struct ProviderReadiness {
    status: ProviderReadinessStatus,
    provider: Option<String>,
    message: String,
}

#[derive(Debug, Clone, Serialize)]
struct ProviderAvailability {
    role: &'static str,
    provider: &'static str,
    label: &'static str,
    status: ProviderReadinessStatus,
    message: String,
}

impl ProviderReadiness {
    fn ready(provider: Option<String>, message: impl Into<String>) -> Self {
        Self {
            status: ProviderReadinessStatus::Ready,
            provider,
            message: message.into(),
        }
    }

    fn offline(provider: Option<String>, message: impl Into<String>) -> Self {
        Self {
            status: ProviderReadinessStatus::Offline,
            provider,
            message: message.into(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RequiredProvider {
    LlamaCpp,
    Ollama,
    Openai,
    Codex,
    Claude,
    Grok,
}

#[tauri::command]
async fn start_job(
    app: AppHandle,
    state: State<'_, DesktopState>,
    file_path: String,
    config: PipelineConfig,
) -> AppResult<DesktopJob> {
    let mut jobs = enqueue_jobs(app, state, vec![file_path], config).await?;
    jobs.pop()
        .ok_or_else(|| AppError::new("queue_empty", "No job was enqueued."))
}

#[tauri::command]
async fn enqueue_jobs(
    app: AppHandle,
    state: State<'_, DesktopState>,
    file_paths: Vec<String>,
    config: PipelineConfig,
) -> AppResult<Vec<DesktopJob>> {
    enqueue_jobs_core(app, state.inner().clone(), file_paths, config).await
}

/// Core enqueue logic shared by the `enqueue_jobs` Tauri command and the
/// single-instance CLI forwarder. Takes an owned `DesktopState` so it can be
/// called from contexts without a `State` extractor (e.g. the single-instance
/// plugin callback).
async fn enqueue_jobs_core(
    app: AppHandle,
    state: DesktopState,
    file_paths: Vec<String>,
    config: PipelineConfig,
) -> AppResult<Vec<DesktopJob>> {
    let paths = normalize_enqueue_paths(file_paths)?;
    for path in &paths {
        validate_input_file(path)?;
    }
    validate_queue_runtime_requirements(&paths, &config)?;

    let enqueued_jobs = build_queued_jobs(&paths, &config);
    let enqueued_ids: HashSet<String> =
        enqueued_jobs.iter().map(|job| job.job_id.clone()).collect();
    let jobs_snapshot = {
        let mut store = state.jobs.lock().await;
        for job in enqueued_jobs.iter().rev() {
            store.jobs.insert(0, job.clone());
        }
        store.jobs.clone()
    };

    if let Err(err) = write_history_file(&jobs_snapshot) {
        let mut store = state.jobs.lock().await;
        store
            .jobs
            .retain(|stored_job| !enqueued_ids.contains(&stored_job.job_id));
        return Err(err);
    }

    for job in &enqueued_jobs {
        tracing::info!(
            target: "summarizer_desktop::queue",
            job_id = %job.job_id,
            file_name = %job.file_name,
            "Job queued"
        );
        app.emit("job:queued", job)
            .map_err(|err| AppError::new("event", err.to_string()))?;
    }
    kick_queue_worker(app, state.clone()).await;

    Ok(enqueued_jobs)
}

/// A parsed headless enqueue request extracted from process arguments.
#[derive(Debug, Clone, PartialEq)]
struct CliEnqueueRequest {
    files: Vec<String>,
    config: PipelineConfig,
}

/// Parse process arguments (excluding `argv[0]`) for a headless enqueue request.
///
/// Recognized flags:
///   --enqueue / -e <path>   file to enqueue (repeatable); also `--enqueue=<path>`
///   --config-json <json>    optional `PipelineConfig` override applied to every
///                           file; also `--config-json=<json>`
///
/// Returns `Ok(None)` when no `--enqueue` argument is present, i.e. a normal GUI
/// launch with no job to forward.
fn parse_cli_enqueue(args: &[String]) -> AppResult<Option<CliEnqueueRequest>> {
    let mut files: Vec<String> = Vec::new();
    let mut config_json: Option<String> = None;
    let mut iter = args.iter();
    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "--enqueue" | "-e" => {
                let value = iter.next().ok_or_else(|| {
                    AppError::new("cli", "--enqueue requires a file path argument.")
                })?;
                files.push(value.clone());
            }
            "--config-json" => {
                let value = iter.next().ok_or_else(|| {
                    AppError::new("cli", "--config-json requires a JSON argument.")
                })?;
                config_json = Some(value.clone());
            }
            other if other.starts_with("--enqueue=") => {
                files.push(other.trim_start_matches("--enqueue=").to_string());
            }
            other if other.starts_with("--config-json=") => {
                config_json = Some(other.trim_start_matches("--config-json=").to_string());
            }
            _ => {}
        }
    }

    if files.is_empty() {
        return Ok(None);
    }

    let config = match config_json {
        Some(raw) => serde_json::from_str(&raw)
            .map_err(|err| AppError::new("cli", format!("Invalid --config-json: {err}")))?,
        None => desktop_default_pipeline_config(),
    };
    Ok(Some(CliEnqueueRequest { files, config }))
}

/// Bring the main window to the foreground. Used when a CLI request is forwarded
/// to the running instance so the user sees the job appear.
fn focus_main_window(app: &AppHandle) {
    let window = app
        .get_webview_window("main")
        .or_else(|| app.webview_windows().into_values().next());
    if let Some(window) = window {
        let _ = window.show();
        let _ = window.unminimize();
        let _ = window.set_focus();
    }
}

/// Enqueue a forwarded CLI request into the running app and focus the window.
/// Runs fire-and-forget, so failures are logged rather than propagated.
async fn handle_cli_enqueue(app: AppHandle, state: DesktopState, request: CliEnqueueRequest) {
    match enqueue_jobs_core(app.clone(), state, request.files, request.config).await {
        Ok(jobs) => tracing::info!(
            target: "summarizer_desktop::cli",
            count = jobs.len(),
            "Enqueued job(s) from CLI request"
        ),
        Err(err) => tracing::error!(
            target: "summarizer_desktop::cli",
            error = %err.message,
            "Failed to enqueue job(s) from CLI request"
        ),
    }
    focus_main_window(&app);
}

/// Run a CLI enqueue request on a dedicated thread that enters the global Tauri
/// runtime via `block_on`. The single-instance callback runs outside the runtime
/// context, so `async_runtime::spawn` there would never be polled; the queue
/// worker that the enqueue kicks is spawned from inside the entered context and
/// continues on the global runtime afterward.
fn spawn_cli_enqueue(app: AppHandle, state: DesktopState, request: CliEnqueueRequest) {
    std::thread::spawn(move || {
        tauri::async_runtime::block_on(handle_cli_enqueue(app, state, request));
    });
}

/// Dispatch a CLI enqueue request parsed from `argv`. Shared by the
/// single-instance forwarder and first-instance startup. The full argv is
/// parsed: any leading program-path element is just an unrecognized token the
/// parser ignores, and the single-instance plugin does not include the
/// executable as `argv[0]` on every platform. Invalid args are logged and
/// ignored so the app still launches/forwards normally.
fn dispatch_cli_enqueue(app: &AppHandle, argv: &[String]) {
    match parse_cli_enqueue(argv) {
        Ok(Some(request)) => {
            spawn_cli_enqueue(
                app.clone(),
                app.state::<DesktopState>().inner().clone(),
                request,
            );
        }
        Ok(None) => focus_main_window(app),
        Err(err) => tracing::error!(
            target: "summarizer_desktop::cli",
            error = %err.message,
            "Ignoring invalid CLI enqueue arguments"
        ),
    }
}

#[tauri::command]
async fn cancel_job(
    app: AppHandle,
    state: State<'_, DesktopState>,
    job_id: String,
) -> AppResult<DesktopJob> {
    let mut store = state.jobs.lock().await;
    let (cloned, event) = cancel_job_in_store(&mut store, &job_id, Utc::now())?;
    let jobs_snapshot = store.jobs.clone();
    drop(store);

    write_history_file(&jobs_snapshot)?;
    app.emit(event.as_str(), &cloned)
        .map_err(|err| AppError::new("event", err.to_string()))?;

    if event == JobCancelEvent::Canceled {
        kick_queue_worker(app, state.inner().clone()).await;
    }
    Ok(cloned)
}

#[tauri::command]
async fn history(state: State<'_, DesktopState>) -> AppResult<Vec<DesktopJob>> {
    Ok(state.jobs.lock().await.jobs.clone())
}

#[tauri::command]
async fn delete_job(state: State<'_, DesktopState>, job_id: String) -> AppResult<Vec<DesktopJob>> {
    let mut store = state.jobs.lock().await;
    if store.active_job_id.as_deref() == Some(job_id.as_str()) {
        return Err(AppError::new(
            "job_in_progress",
            "Cannot delete the active job while its pipeline task is still running.",
        ));
    }
    let Some(job) = store.jobs.iter().find(|job| job.job_id == job_id) else {
        return Err(AppError::new(
            "not_found",
            format!("Job {job_id} not found"),
        ));
    };
    if job.status == DesktopJobStatus::Processing {
        return Err(AppError::new(
            "job_in_progress",
            "Cannot delete a processing job.",
        ));
    }
    remove_job_output_files(&job_id)?;
    store.jobs.retain(|job| job.job_id != job_id);
    store.canceled.remove(&job_id);
    let jobs_snapshot = store.jobs.clone();
    drop(store);
    write_history_file(&jobs_snapshot)?;
    Ok(jobs_snapshot)
}

#[tauri::command]
async fn save_job_markdown(
    state: State<'_, DesktopState>,
    job_id: String,
    output_path: String,
) -> AppResult<()> {
    let output = completed_output(&state, &job_id).await?;
    tokio::fs::write(&output_path, output.to_markdown())
        .await
        .map_err(|err| AppError::storage(format!("Could not save Markdown: {err}")))?;
    Ok(())
}

#[tauri::command]
async fn save_job_json(
    state: State<'_, DesktopState>,
    job_id: String,
    output_path: String,
) -> AppResult<()> {
    let output = completed_output(&state, &job_id).await?;
    let content = serde_json::to_vec_pretty(&output)
        .map_err(|err| AppError::storage(format!("Could not serialize JSON: {err}")))?;
    tokio::fs::write(&output_path, content)
        .await
        .map_err(|err| AppError::storage(format!("Could not save JSON: {err}")))?;
    Ok(())
}

#[tauri::command]
fn load_settings(state: State<'_, DesktopState>) -> AppResult<DesktopSettings> {
    current_settings(&state)
}

#[tauri::command]
fn save_settings(
    state: State<'_, DesktopState>,
    log_state: State<'_, logs::LogState>,
    settings: DesktopSettings,
) -> AppResult<DesktopSettings> {
    log_state.set_enabled(settings.logging.enabled)?;
    log_state.set_level(&settings.logging.level)?;
    write_settings_file(&settings)?;
    let mut guard = state
        .settings
        .write()
        .map_err(|_| AppError::new("settings", "Settings lock is poisoned"))?;
    *guard = settings.clone();
    tracing::info!(
        target: "summarizer_desktop::settings",
        level = %settings.logging.level,
        enabled = settings.logging.enabled,
        "Settings saved"
    );
    Ok(settings)
}

#[tauri::command]
fn settings_file_path() -> AppResult<String> {
    Ok(settings_path()?.display().to_string())
}

#[tauri::command]
async fn provider_readiness(
    state: State<'_, DesktopState>,
    config: PipelineConfig,
) -> AppResult<ProviderReadiness> {
    let settings = current_settings(&state)?;
    Ok(check_provider_readiness(&settings, &config).await)
}

#[tauri::command]
async fn provider_availability(settings: DesktopSettings) -> AppResult<Vec<ProviderAvailability>> {
    Ok(check_visible_provider_availability(&settings).await)
}

fn normalize_enqueue_paths(file_paths: Vec<String>) -> AppResult<Vec<PathBuf>> {
    let mut seen = HashSet::new();
    let mut paths = Vec::new();
    for file_path in file_paths {
        let trimmed = file_path.trim();
        if trimmed.is_empty() {
            continue;
        }
        if seen.insert(trimmed.to_string()) {
            paths.push(PathBuf::from(trimmed));
        }
    }

    if paths.is_empty() {
        return Err(AppError::new(
            "no_files",
            "Choose at least one supported document.",
        ));
    }
    Ok(paths)
}

fn build_queued_jobs(paths: &[PathBuf], config: &PipelineConfig) -> Vec<DesktopJob> {
    let now = Utc::now();
    paths
        .iter()
        .enumerate()
        .map(|(index, path)| {
            let queued_at = now + ChronoDuration::milliseconds(index as i64);
            DesktopJob {
                job_id: Uuid::new_v4().to_string(),
                status: DesktopJobStatus::Queued,
                file_path: path.display().to_string(),
                file_name: file_name(path),
                created_at: queued_at,
                queued_at: Some(queued_at),
                started_at: None,
                completed_at: None,
                duration_ms: None,
                error: None,
                config: Some(config.clone()),
                output: None,
            }
        })
        .collect()
}

async fn kick_queue_worker(app: AppHandle, state: DesktopState) {
    let should_spawn = {
        let mut store = state.jobs.lock().await;
        if store.worker_running
            || !store
                .jobs
                .iter()
                .any(|job| job.status == DesktopJobStatus::Queued)
        {
            false
        } else {
            store.worker_running = true;
            true
        }
    };

    if should_spawn {
        tauri::async_runtime::spawn(async move {
            drain_queue(app, state).await;
        });
    }
}

async fn drain_queue(app: AppHandle, state: DesktopState) {
    while let Some(claim) = claim_next_queued_job(&state).await {
        if let Err(err) = validate_input_file(&claim.path) {
            fail_claimed_job(
                &app,
                &state,
                &claim.job.job_id,
                claim.started_at,
                err.message,
            )
            .await;
            continue;
        }

        let Some(config) = claim.config else {
            fail_claimed_job(
                &app,
                &state,
                &claim.job.job_id,
                claim.started_at,
                "Queued job is missing its pipeline configuration.",
            )
            .await;
            continue;
        };

        let settings = match current_settings_from_desktop(&state) {
            Ok(settings) => settings,
            Err(err) => {
                fail_claimed_job(
                    &app,
                    &state,
                    &claim.job.job_id,
                    claim.started_at,
                    err.message,
                )
                .await;
                continue;
            }
        };
        let readiness = check_provider_readiness(&settings, &config).await;
        if readiness.status == ProviderReadinessStatus::Offline {
            fail_claimed_job(
                &app,
                &state,
                &claim.job.job_id,
                claim.started_at,
                readiness.message,
            )
            .await;
            continue;
        }
        if let Err(err) = prepare_runtime_environment(&app, &claim.path) {
            fail_claimed_job(
                &app,
                &state,
                &claim.job.job_id,
                claim.started_at,
                err.message,
            )
            .await;
            continue;
        }
        let provider_config = pipeline_provider_config(&settings);

        let _ = app.emit("job:started", &claim.job);
        run_job_task(
            app.clone(),
            state.clone(),
            claim.job.job_id.clone(),
            claim.path,
            config,
            provider_config,
            claim.started_at,
        )
        .await;
    }
}

async fn claim_next_queued_job(state: &DesktopState) -> Option<QueueClaim> {
    let (job, jobs_snapshot) = {
        let mut store = state.jobs.lock().await;
        match claim_next_queued_in_store(&mut store, Utc::now()) {
            Some(job) => (job, store.jobs.clone()),
            None => {
                store.worker_running = false;
                return None;
            }
        }
    };
    if let Err(err) = write_history_file(&jobs_snapshot) {
        tracing::warn!(
            target: "summarizer_desktop::queue",
            error = %err.message,
            "Could not persist history after claiming job"
        );
    }
    Some(QueueClaim {
        path: PathBuf::from(&job.file_path),
        config: job.config.clone(),
        started_at: job.started_at.unwrap_or(job.created_at),
        job,
    })
}

fn claim_next_queued_in_store(
    store: &mut JobStore,
    started_at: DateTime<Utc>,
) -> Option<DesktopJob> {
    if store.active_job_id.is_some() {
        return None;
    }
    let index = next_queued_job_index(&store.jobs)?;
    let job = &mut store.jobs[index];
    job.status = DesktopJobStatus::Processing;
    job.started_at = Some(started_at);
    job.completed_at = None;
    job.duration_ms = None;
    job.error = None;
    job.output = None;
    store.active_job_id = Some(job.job_id.clone());
    Some(job.clone())
}

fn cancel_job_in_store(
    store: &mut JobStore,
    job_id: &str,
    now: DateTime<Utc>,
) -> AppResult<(DesktopJob, JobCancelEvent)> {
    let active_matches = store.active_job_id.as_deref() == Some(job_id);
    let job_index = store
        .jobs
        .iter()
        .position(|job| job.job_id == job_id)
        .ok_or_else(|| AppError::new("not_found", format!("Job {job_id} not found")))?;
    let status = store.jobs[job_index].status;

    match status {
        DesktopJobStatus::Queued => {
            let job = &mut store.jobs[job_index];
            job.status = DesktopJobStatus::Canceled;
            job.completed_at = Some(now);
            job.duration_ms = Some(0);
            job.error = Some("Job canceled before processing started.".to_string());
            Ok((job.clone(), JobCancelEvent::Canceled))
        }
        DesktopJobStatus::Processing if active_matches => {
            store.canceled.insert(job_id.to_string());
            let job = &mut store.jobs[job_index];
            job.error = Some(
                "Cancellation requested. The running pipeline result will be discarded."
                    .to_string(),
            );
            Ok((job.clone(), JobCancelEvent::Updated))
        }
        DesktopJobStatus::Processing => Err(AppError::new(
            "not_active",
            "Only the active processing job can be canceled.",
        )),
        DesktopJobStatus::Completed | DesktopJobStatus::Failed | DesktopJobStatus::Canceled => {
            Err(AppError::new(
                "terminal_job",
                "This job has already reached a terminal state.",
            ))
        }
    }
}

fn next_queued_job_index(jobs: &[DesktopJob]) -> Option<usize> {
    jobs.iter()
        .enumerate()
        .filter(|(_, job)| job.status == DesktopJobStatus::Queued)
        .min_by(|(left_index, left), (right_index, right)| {
            let left_queued_at = left.queued_at.unwrap_or(left.created_at);
            let right_queued_at = right.queued_at.unwrap_or(right.created_at);
            left_queued_at
                .cmp(&right_queued_at)
                .then_with(|| left.created_at.cmp(&right.created_at))
                .then_with(|| left_index.cmp(right_index))
        })
        .map(|(index, _)| index)
}

async fn fail_claimed_job(
    app: &AppHandle,
    state: &DesktopState,
    job_id: &str,
    started_at: DateTime<Utc>,
    message: impl Into<String>,
) {
    let completed_at = Utc::now();
    let duration_ms = (completed_at - started_at).num_milliseconds().max(0) as u64;
    let message = message.into();
    let (failed_job, jobs_snapshot) = {
        let mut store = state.jobs.lock().await;
        let failed_job = store
            .jobs
            .iter_mut()
            .find(|job| job.job_id == job_id)
            .map(|job| {
                job.status = DesktopJobStatus::Failed;
                job.completed_at = Some(completed_at);
                job.duration_ms = Some(duration_ms);
                job.error = Some(message);
                job.output = None;
                job.clone()
            });
        if store.active_job_id.as_deref() == Some(job_id) {
            store.active_job_id = None;
        }
        (failed_job, store.jobs.clone())
    };

    if let Err(err) = write_history_file(&jobs_snapshot) {
        tracing::warn!(
            target: "summarizer_desktop::queue",
            job_id,
            error = %err.message,
            "Could not persist history after failing job"
        );
    }
    if let Some(job) = failed_job {
        let _ = app.emit("job:failed", job);
    }
}

async fn run_job_task(
    app: AppHandle,
    state: DesktopState,
    job_id: String,
    path: PathBuf,
    config: PipelineConfig,
    provider_config: PipelineProviderConfig,
    started_at: DateTime<Utc>,
) {
    let progress_app = app.clone();
    let progress_job_id = job_id.clone();
    let progress_file_name = file_name(&path);
    tracing::info!(
        target: "summarizer_desktop::pipeline",
        job_id = %job_id,
        file_name = %progress_file_name,
        "Job started"
    );
    let pipeline = Pipeline::with_provider_config(provider_config);
    let result = pipeline
        .run_path_with_progress(&job_id, &path, &config, move |progress| {
            let payload =
                DesktopJobProgress::from_pipeline(&progress_job_id, &progress_file_name, progress);
            tracing::debug!(
                target: "summarizer_desktop::pipeline",
                job_id = %payload.job_id,
                stage = %payload.stage,
                progress = payload.progress,
                message = %payload.message,
                "Pipeline progress"
            );
            let _ = progress_app.emit("job:progress", payload);
        })
        .await;
    let completed_at = Utc::now();
    let duration_ms = (completed_at - started_at).num_milliseconds().max(0) as u64;

    let (event_name, event_job, jobs_snapshot) = {
        let mut store = state.jobs.lock().await;
        let canceled = store.canceled.remove(&job_id);
        let mut event_name = "job:failed";
        let mut event_job = None;

        if let Some(job) = store.jobs.iter_mut().find(|job| job.job_id == job_id) {
            job.completed_at = Some(completed_at);
            job.duration_ms = Some(duration_ms);
            if canceled || job.status == DesktopJobStatus::Canceled {
                job.status = DesktopJobStatus::Canceled;
                job.output = None;
                job.error =
                    Some("Job canceled. Completed pipeline result was discarded.".to_string());
                event_name = "job:canceled";
            } else {
                match result {
                    Ok(output) => match write_job_output_files(&job.job_id, &output) {
                        Ok(()) => {
                            job.status = DesktopJobStatus::Completed;
                            job.output = Some(output);
                            job.error = None;
                            event_name = "job:completed";
                        }
                        Err(err) => {
                            job.status = DesktopJobStatus::Failed;
                            job.output = None;
                            job.error = Some(err.message);
                            event_name = "job:failed";
                        }
                    },
                    Err(err) => {
                        job.status = DesktopJobStatus::Failed;
                        job.output = None;
                        job.error = Some(err.to_string());
                        event_name = "job:failed";
                    }
                }
            }
            event_job = Some(job.clone());
        }

        if store.active_job_id.as_deref() == Some(job_id.as_str()) {
            store.active_job_id = None;
        }

        (event_name, event_job, store.jobs.clone())
    };

    if let Err(err) = write_history_file(&jobs_snapshot) {
        tracing::warn!(
            target: "summarizer_desktop::queue",
            job_id = %job_id,
            error = %err.message,
            "Could not persist history after job completion"
        );
    }

    if let Some(job) = event_job {
        tracing::info!(
            target: "summarizer_desktop::pipeline",
            job_id = %job.job_id,
            status = ?job.status,
            duration_ms = job.duration_ms.unwrap_or_default(),
            "Job finished"
        );
        if event_name == "job:completed" {
            let payload = DesktopJobProgress::completed(&job.job_id, &job.file_name);
            let _ = app.emit("job:progress", payload);
        }
        let _ = app.emit(event_name, job);
    }
}

async fn completed_output(
    state: &State<'_, DesktopState>,
    job_id: &str,
) -> AppResult<DocumentOutput> {
    let store = state.jobs.lock().await;
    let job = store
        .jobs
        .iter()
        .find(|job| job.job_id == job_id)
        .ok_or_else(|| AppError::new("not_found", format!("Job {job_id} not found")))?;
    job.output.clone().ok_or_else(|| {
        AppError::new(
            "output_unavailable",
            "Output is not available for this job.",
        )
    })
}

fn validate_input_file(path: &Path) -> AppResult<()> {
    if !path.is_file() {
        return Err(AppError::new(
            "invalid_file",
            format!("File not found: {}", path.display()),
        ));
    }

    match path
        .extension()
        .and_then(|extension| extension.to_str())
        .map(|extension| extension.to_ascii_lowercase())
        .as_deref()
    {
        Some("pdf" | "pptx" | "docx" | "txt" | "md" | "markdown") => Ok(()),
        _ => Err(AppError::new(
            "invalid_file_type",
            "Choose a PDF, PPTX, DOCX, TXT, or MD file.",
        )),
    }
}

fn validate_queue_runtime_requirements(
    paths: &[PathBuf],
    config: &PipelineConfig,
) -> AppResult<()> {
    validate_queue_runtime_requirements_with_renderer(paths, config, soffice_path().is_some())
}

fn validate_queue_runtime_requirements_with_renderer(
    paths: &[PathBuf],
    config: &PipelineConfig,
    renderer_available: bool,
) -> AppResult<()> {
    if paths.iter().any(|path| is_pptx_path(path))
        && pptx_slide_screenshots_required(config)
        && !renderer_available
    {
        return Err(AppError::new(
            "libreoffice_missing",
            "PPTX slide screenshots require LibreOffice/soffice. Install LibreOffice or enable Advanced Mode > Skip Slide Screenshots.",
        ));
    }
    Ok(())
}

fn is_pptx_path(path: &Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| extension.eq_ignore_ascii_case("pptx"))
}

fn pptx_slide_screenshots_required(config: &PipelineConfig) -> bool {
    config.run_extraction
        && !config.skip_images
        && !config.text_only
        && !config.extract_only
        && config.vision_mode != VisionMode::None
}

fn soffice_path() -> Option<PathBuf> {
    resolve_soffice()
}

fn prepare_runtime_environment(app: &AppHandle, path: &Path) -> AppResult<()> {
    let requires_pdfium = path
        .extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| {
            extension.eq_ignore_ascii_case("pdf") || extension.eq_ignore_ascii_case("pptx")
        });
    if requires_pdfium {
        let pdfium = bundled_pdfium_path(app)?;
        configure_pdfium_library_path(pdfium).map_err(AppError::from)?;
    }
    Ok(())
}

fn bundled_pdfium_path(app: &AppHandle) -> AppResult<PathBuf> {
    let library_name = pdfium_library_name();
    let resource_dir = app
        .path()
        .resource_dir()
        .map_err(|err| AppError::new("pdfium", err.to_string()))?;
    let primary = resource_dir
        .join("resources")
        .join("pdfium")
        .join(library_name);
    let candidates = [
        primary.clone(),
        resource_dir.join("pdfium").join(library_name),
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("resources")
            .join("pdfium")
            .join(library_name),
    ];

    candidates
        .iter()
        .find(|candidate| candidate.is_file())
        .cloned()
        .ok_or_else(|| {
            AppError::new(
                "pdfium_missing",
                format!(
                    "PDF processing requires PDFium. Bundled library not found at {}.",
                    primary.display()
                ),
            )
        })
}

fn pdfium_library_name() -> &'static str {
    #[cfg(target_os = "windows")]
    {
        "pdfium.dll"
    }
    #[cfg(target_os = "macos")]
    {
        "libpdfium.dylib"
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        "libpdfium.so"
    }
}

fn pipeline_provider_config(settings: &DesktopSettings) -> PipelineProviderConfig {
    let providers = &settings.providers;
    let openai_model = setting_or(&providers.openai.model, "gpt-4.1-mini");
    let llama_model = setting_or(&providers.llama_cpp.model, "model.gguf");
    let ollama_model = setting_or(&providers.ollama.model, "llama3.2");

    PipelineProviderConfig {
        openai: HttpProviderConfig {
            base_url: setting_or(&providers.openai.base_url, "https://api.openai.com/v1"),
            api_key: optional_setting(&providers.openai.api_key),
            model: openai_model.clone(),
            model_2: setting_or(&providers.openai.model_2, &openai_model),
            model_3: setting_or(&providers.openai.model_3, &openai_model),
            vision_model: setting_or(&providers.openai.vision_model, "gpt-4.1-mini"),
        },
        llama_cpp: LlamaCppProviderConfig {
            base_url: setting_or(&providers.llama_cpp.base_url, "http://localhost:11440/v1"),
            vision_base_url: setting_or(
                fallback_url(
                    &providers.llama_cpp.vision_base_url,
                    &providers.llama_cpp.base_url,
                ),
                "http://localhost:11440/v1",
            ),
            api_key: optional_setting(&providers.llama_cpp.api_key),
            model: llama_model.clone(),
            model_2: setting_or(&providers.llama_cpp.model_2, &llama_model),
            model_3: setting_or(&providers.llama_cpp.model_3, &llama_model),
            vision_model: setting_or(&providers.llama_cpp.vision_model, "model.gguf"),
        },
        ollama: OllamaProviderConfig {
            openai_base_url: setting_or(
                &providers.ollama.openai_base_url,
                "http://localhost:11434/v1",
            ),
            api_key: optional_setting(&providers.ollama.api_key),
            model: ollama_model.clone(),
            model_2: setting_or(&providers.ollama.model_2, &ollama_model),
            model_3: setting_or(&providers.ollama.model_3, &ollama_model),
            vision_model: setting_or(&providers.ollama.vision_model, "llava"),
        },
        gemini: GeminiProviderConfig {
            base_url: "https://generativelanguage.googleapis.com".to_string(),
            api_key: None,
            vision_model: "gemini-2.5-flash".to_string(),
        },
        codex: cli_runtime_config(&providers.codex, codex_cli_args),
        claude: cli_runtime_config(&providers.claude, claude_cli_args),
        grok: cli_runtime_config(&providers.grok, grok_cli_args),
    }
}

fn setting_or(value: &str, default: &str) -> String {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        default.to_string()
    } else {
        trimmed.to_string()
    }
}

fn optional_setting(value: &str) -> Option<String> {
    let trimmed = value.trim();
    (!trimmed.is_empty()).then(|| trimmed.to_string())
}

fn cli_runtime_config(
    settings: &CliSettings,
    args: fn(&CliSettings) -> String,
) -> CliRuntimeConfig {
    CliRuntimeConfig {
        executable: resolved_cli_executable_value(&settings.executable),
        args: split_cli_args(&args(settings)),
        timeout_seconds: settings.timeout_seconds,
        retries: summarizer_pipeline::DEFAULT_CLI_RETRIES,
    }
}

fn codex_cli_args(settings: &CliSettings) -> String {
    append_cli_args(
        vec![
            "-c".to_string(),
            format!(
                "model_reasoning_effort={}",
                normalized_reasoning_effort(&settings.reasoning_effort)
            ),
        ],
        &settings.args,
    )
}

fn claude_cli_args(settings: &CliSettings) -> String {
    append_cli_args(
        vec![
            "--effort".to_string(),
            normalized_reasoning_effort(&settings.reasoning_effort).to_string(),
        ],
        &settings.args,
    )
}

fn grok_cli_args(settings: &CliSettings) -> String {
    append_cli_args(
        vec![
            "--no-auto-update".to_string(),
            "--sandbox".to_string(),
            "read-only".to_string(),
            "--disable-web-search".to_string(),
            "--no-subagents".to_string(),
            "--no-plan".to_string(),
            "--always-approve".to_string(),
            "--tools=".to_string(),
        ],
        &settings.args,
    )
}

fn append_cli_args(mut args: Vec<String>, custom_args: &str) -> String {
    args.extend(
        custom_args
            .split_whitespace()
            .filter(|arg| !arg.is_empty())
            .map(ToString::to_string),
    );
    args.join(" ")
}

fn split_cli_args(args: &str) -> Vec<String> {
    args.split_whitespace()
        .filter(|arg| !arg.is_empty())
        .map(ToString::to_string)
        .collect()
}

fn normalized_reasoning_effort(value: &str) -> &str {
    match value.trim() {
        "low" => "low",
        "high" => "high",
        "xhigh" => "xhigh",
        "max" => "max",
        _ => "medium",
    }
}

async fn check_visible_provider_availability(
    settings: &DesktopSettings,
) -> Vec<ProviderAvailability> {
    let mut checks = Vec::new();
    let visibility = &settings.provider_visibility;

    if visibility.vision.llama_cpp || visibility.classifier.llama_cpp {
        push_availability_check(&mut checks, "vision", RequiredProvider::LlamaCpp);
    }
    if visibility.vision.ollama || visibility.classifier.ollama {
        push_availability_check(&mut checks, "vision", RequiredProvider::Ollama);
    }
    if visibility.vision.openai || visibility.classifier.openai {
        push_availability_check(&mut checks, "vision", RequiredProvider::Openai);
    }
    if visibility.vision.codex || visibility.classifier.codex {
        push_availability_check(&mut checks, "vision", RequiredProvider::Codex);
    }
    if visibility.vision.claude || visibility.classifier.claude {
        push_availability_check(&mut checks, "vision", RequiredProvider::Claude);
    }
    if visibility.vision.grok || visibility.classifier.grok {
        push_availability_check(&mut checks, "vision", RequiredProvider::Grok);
    }

    if visibility.summarizer.llama_cpp {
        push_availability_check(&mut checks, "summarizer", RequiredProvider::LlamaCpp);
    }
    if visibility.summarizer.ollama {
        push_availability_check(&mut checks, "summarizer", RequiredProvider::Ollama);
    }
    if visibility.summarizer.openai {
        push_availability_check(&mut checks, "summarizer", RequiredProvider::Openai);
    }
    if visibility.summarizer.codex {
        push_availability_check(&mut checks, "summarizer", RequiredProvider::Codex);
    }
    if visibility.summarizer.claude {
        push_availability_check(&mut checks, "summarizer", RequiredProvider::Claude);
    }
    if visibility.summarizer.grok {
        push_availability_check(&mut checks, "summarizer", RequiredProvider::Grok);
    }

    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(3))
        .build();
    let mut availability = Vec::with_capacity(checks.len());
    for (role, provider) in checks {
        let result = match &client {
            Ok(client) => check_provider_for_role(client, settings, role, provider).await,
            Err(err) => Err(format!(
                "Could not initialize provider availability check: {err}"
            )),
        };
        availability.push(ProviderAvailability {
            role,
            provider: provider.value(),
            label: provider.label(),
            status: if result.is_ok() {
                ProviderReadinessStatus::Ready
            } else {
                ProviderReadinessStatus::Offline
            },
            message: result.unwrap_or_else(|message| message),
        });
    }
    availability
}

fn push_availability_check(
    checks: &mut Vec<(&'static str, RequiredProvider)>,
    role: &'static str,
    provider: RequiredProvider,
) {
    if !checks
        .iter()
        .any(|(stored_role, stored_provider)| *stored_role == role && *stored_provider == provider)
    {
        checks.push((role, provider));
    }
}

async fn check_provider_for_role(
    client: &reqwest::Client,
    settings: &DesktopSettings,
    role: &str,
    provider: RequiredProvider,
) -> Result<String, String> {
    match (role, provider) {
        ("vision", RequiredProvider::LlamaCpp) => {
            check_llama_cpp_provider(
                client,
                provider.label(),
                fallback_url(
                    &settings.providers.llama_cpp.vision_base_url,
                    &settings.providers.llama_cpp.base_url,
                ),
                optional_secret(&settings.providers.llama_cpp.api_key),
            )
            .await
        }
        ("summarizer", RequiredProvider::LlamaCpp) => {
            check_llama_cpp_provider(
                client,
                provider.label(),
                &settings.providers.llama_cpp.base_url,
                optional_secret(&settings.providers.llama_cpp.api_key),
            )
            .await
        }
        (_, provider) => check_required_provider(client, settings, provider).await,
    }
    .map(|()| format!("{} is ready.", provider.label()))
}

fn fallback_url<'a>(primary: &'a str, fallback: &'a str) -> &'a str {
    if primary.trim().is_empty() {
        fallback
    } else {
        primary
    }
}

async fn check_provider_readiness(
    settings: &DesktopSettings,
    config: &PipelineConfig,
) -> ProviderReadiness {
    let required = match required_providers(config) {
        Ok(required) => required,
        Err(readiness) => return readiness,
    };
    if required.is_empty() {
        return ProviderReadiness::ready(
            None,
            "Ready. This configuration does not require an AI provider.",
        );
    }

    let client = match reqwest::Client::builder()
        .timeout(Duration::from_secs(3))
        .build()
    {
        Ok(client) => client,
        Err(err) => {
            return ProviderReadiness::offline(
                None,
                format!("Could not initialize provider readiness check: {err}"),
            )
        }
    };

    for provider in &required {
        if let Err(message) = check_required_provider(&client, settings, *provider).await {
            return ProviderReadiness::offline(Some(provider.label().to_string()), message);
        }
    }

    let provider = if required.len() == 1 {
        Some(required[0].label().to_string())
    } else {
        Some("Selected providers".to_string())
    };
    ProviderReadiness::ready(
        provider,
        if required.len() == 1 {
            format!("{} is ready.", required[0].label())
        } else {
            "Selected providers are ready.".to_string()
        },
    )
}

fn required_providers(config: &PipelineConfig) -> Result<Vec<RequiredProvider>, ProviderReadiness> {
    let mut providers = Vec::new();

    if config.vision_mode != VisionMode::None && !config.extract_only {
        let extractor = resolve_cli_vision_mode_for_readiness(
            config.vision_extractor_mode.unwrap_or(config.vision_mode),
            config.vision_cli_provider,
        );
        push_vision_provider(&mut providers, extractor)?;

        if !config.vision_skip_classification {
            let classifier = resolve_cli_vision_mode_for_readiness(
                config.vision_classifier_mode.unwrap_or(config.vision_mode),
                config.vision_cli_provider,
            );
            push_vision_provider(&mut providers, classifier)?;
        }
    }

    if config.run_summarization
        && config.summarizer_mode != SummarizerMode::Skip
        && !config.extract_only
    {
        push_provider(
            &mut providers,
            summarizer_required_provider(resolve_summarizer_provider_for_readiness(config)),
        );
    }

    Ok(providers)
}

fn push_vision_provider(
    providers: &mut Vec<RequiredProvider>,
    mode: VisionMode,
) -> Result<(), ProviderReadiness> {
    let provider = match mode {
        VisionMode::None => return Ok(()),
        VisionMode::Deepseek => {
            return Err(ProviderReadiness::offline(
                Some("Deepseek".to_string()),
                "Deepseek vision is not supported by the desktop pipeline.",
            ))
        }
        VisionMode::Gemini => {
            return Err(ProviderReadiness::offline(
                Some("Gemini".to_string()),
                "Gemini has been removed from the desktop provider list. Choose another vision provider.",
            ))
        }
        VisionMode::Openai => RequiredProvider::Openai,
        VisionMode::Ollama => RequiredProvider::Ollama,
        VisionMode::LlamaCpp => RequiredProvider::LlamaCpp,
        VisionMode::Codex => RequiredProvider::Codex,
        VisionMode::Claude => RequiredProvider::Claude,
        VisionMode::Grok => RequiredProvider::Grok,
    };
    push_provider(providers, provider);
    Ok(())
}

fn push_provider(providers: &mut Vec<RequiredProvider>, provider: RequiredProvider) {
    if !providers.contains(&provider) {
        providers.push(provider);
    }
}

fn resolve_cli_vision_mode_for_readiness(
    mode: VisionMode,
    cli_provider: Option<CliProvider>,
) -> VisionMode {
    match (mode, cli_provider) {
        (VisionMode::Codex | VisionMode::Claude | VisionMode::Grok, Some(CliProvider::Codex)) => {
            VisionMode::Codex
        }
        (VisionMode::Codex | VisionMode::Claude | VisionMode::Grok, Some(CliProvider::Claude)) => {
            VisionMode::Claude
        }
        (VisionMode::Codex | VisionMode::Claude | VisionMode::Grok, Some(CliProvider::Grok)) => {
            VisionMode::Grok
        }
        _ => mode,
    }
}

fn resolve_summarizer_provider_for_readiness(config: &PipelineConfig) -> SummarizerProvider {
    match (config.summarizer_provider, config.summarizer_cli_provider) {
        (
            SummarizerProvider::Codex | SummarizerProvider::Claude | SummarizerProvider::Grok,
            Some(CliProvider::Codex),
        ) => SummarizerProvider::Codex,
        (
            SummarizerProvider::Codex | SummarizerProvider::Claude | SummarizerProvider::Grok,
            Some(CliProvider::Claude),
        ) => SummarizerProvider::Claude,
        (
            SummarizerProvider::Codex | SummarizerProvider::Claude | SummarizerProvider::Grok,
            Some(CliProvider::Grok),
        ) => SummarizerProvider::Grok,
        _ => config.summarizer_provider,
    }
}

fn summarizer_required_provider(provider: SummarizerProvider) -> RequiredProvider {
    match provider {
        SummarizerProvider::LlamaCpp => RequiredProvider::LlamaCpp,
        SummarizerProvider::Ollama => RequiredProvider::Ollama,
        SummarizerProvider::Openai => RequiredProvider::Openai,
        SummarizerProvider::Codex => RequiredProvider::Codex,
        SummarizerProvider::Claude => RequiredProvider::Claude,
        SummarizerProvider::Grok => RequiredProvider::Grok,
    }
}

async fn check_required_provider(
    client: &reqwest::Client,
    settings: &DesktopSettings,
    provider: RequiredProvider,
) -> Result<(), String> {
    match provider {
        RequiredProvider::LlamaCpp => {
            check_llama_cpp_provider(
                client,
                provider.label(),
                &settings.providers.llama_cpp.base_url,
                optional_secret(&settings.providers.llama_cpp.api_key),
            )
            .await
        }
        RequiredProvider::Ollama => {
            check_openai_compatible_provider(
                client,
                provider.label(),
                &settings.providers.ollama.openai_base_url,
                optional_secret(&settings.providers.ollama.api_key),
            )
            .await
        }
        RequiredProvider::Openai => {
            let api_key = required_secret(provider.label(), &settings.providers.openai.api_key)?;
            check_openai_compatible_provider(
                client,
                provider.label(),
                &settings.providers.openai.base_url,
                Some(api_key),
            )
            .await
        }
        RequiredProvider::Codex => {
            check_cli_provider(provider.label(), &settings.providers.codex.executable)
        }
        RequiredProvider::Claude => {
            check_cli_provider(provider.label(), &settings.providers.claude.executable)
        }
        RequiredProvider::Grok => {
            check_cli_provider(provider.label(), &settings.providers.grok.executable)
        }
    }
}

async fn check_openai_compatible_provider(
    client: &reqwest::Client,
    label: &str,
    base_url: &str,
    api_key: Option<&str>,
) -> Result<(), String> {
    let url = provider_models_url(base_url)?;
    let mut request = client.get(url);
    if let Some(api_key) = api_key {
        request = request.bearer_auth(api_key);
    }
    let response = request
        .send()
        .await
        .map_err(|err| format!("{label} is offline or unreachable: {err}"))?;
    if response.status().is_success() {
        Ok(())
    } else {
        Err(format!(
            "{label} returned {} from its models endpoint.",
            response.status()
        ))
    }
}

async fn check_llama_cpp_provider(
    client: &reqwest::Client,
    label: &str,
    base_url: &str,
    api_key: Option<&str>,
) -> Result<(), String> {
    match check_openai_compatible_provider(client, label, base_url, api_key).await {
        Ok(()) => Ok(()),
        Err(models_error) => {
            let health_url = provider_health_url(base_url)?;
            let mut request = client.get(health_url);
            if let Some(api_key) = api_key {
                request = request.bearer_auth(api_key);
            }
            let response = request
                .send()
                .await
                .map_err(|err| format!("{models_error}; health check also failed: {err}"))?;
            if response.status().is_success() {
                Ok(())
            } else {
                Err(format!(
                    "{models_error}; health check returned {}.",
                    response.status()
                ))
            }
        }
    }
}

fn provider_models_url(base_url: &str) -> Result<reqwest::Url, String> {
    reqwest::Url::parse(&format!("{}/models", base_url.trim().trim_end_matches('/')))
        .map_err(|err| format!("Provider base URL is invalid: {err}"))
}

fn provider_health_url(base_url: &str) -> Result<reqwest::Url, String> {
    let mut url = reqwest::Url::parse(base_url.trim().trim_end_matches('/'))
        .map_err(|err| format!("Provider base URL is invalid: {err}"))?;
    let base_path = url.path().trim_end_matches('/');
    let health_root = base_path.strip_suffix("/v1").unwrap_or(base_path);
    let health_path = if health_root.is_empty() {
        "/health".to_string()
    } else {
        format!("{}/health", health_root.trim_end_matches('/'))
    };
    url.set_path(&health_path);
    url.set_query(None);
    url.set_fragment(None);
    Ok(url)
}

fn check_cli_provider(label: &str, executable: &str) -> Result<(), String> {
    let executable = executable.trim();
    if executable.is_empty() {
        return Err(format!("{label} executable is not configured."));
    }
    let resolved = resolve_cli_executable(executable).ok_or_else(|| {
        format!(
            "{label} executable '{executable}' was not found. Set an absolute path in Settings or install it in a standard CLI location."
        )
    })?;
    let mut command = Command::new(&resolved);
    suppress_command_window(&mut command);
    command
        .arg("--version")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    if let Some(path) = cli_search_path() {
        command.env("PATH", path);
    }
    let status = command.status().map_err(|err| {
        format!(
            "{label} executable '{}' could not be started: {err}",
            resolved.display()
        )
    })?;
    if status.success() {
        return Ok(());
    }
    Err(format!(
        "{label} executable '{}' returned {status} from --version.",
        resolved.display()
    ))
}

#[cfg(test)]
fn command_available(executable: &str) -> bool {
    resolve_cli_executable(executable).is_some()
}

fn resolved_cli_executable_value(executable: &str) -> String {
    resolve_cli_executable(executable)
        .map(|path| path.display().to_string())
        .unwrap_or_else(|| executable.trim().to_string())
}

fn resolve_cli_executable(executable: &str) -> Option<PathBuf> {
    resolve_cli_executable_with_extra_dirs(executable, &desktop_cli_extra_dirs())
}

fn cli_search_path() -> Option<std::ffi::OsString> {
    cli_search_path_with_extra_dirs(&desktop_cli_extra_dirs())
}

fn desktop_cli_extra_dirs() -> Vec<PathBuf> {
    vec![
        PathBuf::from("/Applications/Codex.app/Contents/Resources"),
        PathBuf::from("/Applications/cmux.app/Contents/Resources/bin"),
    ]
}

fn required_secret<'a>(label: &str, value: &'a str) -> Result<&'a str, String> {
    optional_secret(value).ok_or_else(|| format!("{label} API key is not configured."))
}

fn optional_secret(value: &str) -> Option<&str> {
    let value = value.trim();
    if value.is_empty() {
        None
    } else {
        Some(value)
    }
}

impl RequiredProvider {
    fn value(self) -> &'static str {
        match self {
            RequiredProvider::LlamaCpp => "llama_cpp",
            RequiredProvider::Ollama => "ollama",
            RequiredProvider::Openai => "openai",
            RequiredProvider::Codex => "codex",
            RequiredProvider::Claude => "claude",
            RequiredProvider::Grok => "grok",
        }
    }

    fn label(self) -> &'static str {
        match self {
            RequiredProvider::LlamaCpp => "llama.cpp",
            RequiredProvider::Ollama => "Ollama",
            RequiredProvider::Openai => "OpenAI",
            RequiredProvider::Codex => "Codex CLI",
            RequiredProvider::Claude => "Claude CLI",
            RequiredProvider::Grok => "Grok CLI",
        }
    }
}

fn current_settings(state: &State<'_, DesktopState>) -> AppResult<DesktopSettings> {
    current_settings_from_desktop(state.inner())
}

fn current_settings_from_desktop(state: &DesktopState) -> AppResult<DesktopSettings> {
    state
        .settings
        .read()
        .map_err(|_| AppError::new("settings", "Settings lock is poisoned"))
        .map(|settings| settings.clone())
}

fn load_settings_from_disk() -> DesktopSettings {
    let default_settings = DesktopSettings::default();
    let Ok(path) = settings_path() else {
        return default_settings;
    };
    match fs::read_to_string(&path) {
        Ok(content) => serde_json::from_str(&content).unwrap_or(default_settings),
        Err(err) if err.kind() == io::ErrorKind::NotFound => {
            let _ = write_settings_file(&default_settings);
            default_settings
        }
        Err(_) => default_settings,
    }
}

fn load_history_from_disk() -> Vec<DesktopJob> {
    let Ok(path) = history_path() else {
        return Vec::new();
    };
    match fs::read_to_string(&path) {
        Ok(content) => {
            let mut jobs: Vec<DesktopJob> = serde_json::from_str(&content).unwrap_or_default();
            let mut normalized = false;
            for job in &mut jobs {
                let embedded_output = job.output.take();
                if job.status == DesktopJobStatus::Processing {
                    job.status = DesktopJobStatus::Canceled;
                    job.output = None;
                    job.error = Some("App closed before processing completed.".to_string());
                    normalized = true;
                    continue;
                }

                if job.status != DesktopJobStatus::Completed {
                    job.output = None;
                    continue;
                }

                match read_job_output_file(&job.job_id) {
                    Ok(Some(output)) => job.output = Some(output),
                    Ok(None) => {
                        if let Some(output) = embedded_output {
                            if write_job_output_files(&job.job_id, &output).is_ok() {
                                job.output = Some(output);
                                normalized = true;
                            }
                        }
                    }
                    Err(_) => {
                        job.output = embedded_output;
                    }
                }
            }
            if normalized {
                let _ = write_history_file(&jobs);
            }
            jobs
        }
        Err(err) if err.kind() == io::ErrorKind::NotFound => {
            let _ = write_history_file(&[]);
            Vec::new()
        }
        Err(_) => Vec::new(),
    }
}

fn write_settings_file(settings: &DesktopSettings) -> AppResult<()> {
    let path = settings_path()?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|err| {
            AppError::storage(format!("Could not create settings directory: {err}"))
        })?;
    }

    let content = serde_json::to_vec_pretty(settings)
        .map_err(|err| AppError::storage(format!("Could not serialize settings: {err}")))?;

    #[cfg(unix)]
    {
        use std::io::Write;
        use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
        let mut file = fs::OpenOptions::new()
            .create(true)
            .truncate(true)
            .write(true)
            .mode(0o600)
            .open(&path)
            .map_err(|err| AppError::storage(format!("Could not open settings file: {err}")))?;
        file.write_all(&content)
            .map_err(|err| AppError::storage(format!("Could not write settings file: {err}")))?;
        file.sync_all()
            .map_err(|err| AppError::storage(format!("Could not sync settings file: {err}")))?;
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).map_err(|err| {
            AppError::storage(format!("Could not set settings permissions: {err}"))
        })?;
    }

    #[cfg(not(unix))]
    {
        fs::write(&path, content)
            .map_err(|err| AppError::storage(format!("Could not write settings file: {err}")))?;
    }

    Ok(())
}

fn write_history_file(jobs: &[DesktopJob]) -> AppResult<()> {
    let path = history_path()?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|err| {
            AppError::storage(format!("Could not create history directory: {err}"))
        })?;
    }

    let history_jobs: Vec<DesktopJob> = jobs
        .iter()
        .cloned()
        .map(|mut job| {
            job.output = None;
            job
        })
        .collect();
    let content = serde_json::to_vec_pretty(&history_jobs)
        .map_err(|err| AppError::storage(format!("Could not serialize history: {err}")))?;

    #[cfg(unix)]
    {
        use std::io::Write;
        use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
        let mut file = fs::OpenOptions::new()
            .create(true)
            .truncate(true)
            .write(true)
            .mode(0o600)
            .open(&path)
            .map_err(|err| AppError::storage(format!("Could not open history file: {err}")))?;
        file.write_all(&content)
            .map_err(|err| AppError::storage(format!("Could not write history file: {err}")))?;
        file.sync_all()
            .map_err(|err| AppError::storage(format!("Could not sync history file: {err}")))?;
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).map_err(|err| {
            AppError::storage(format!("Could not set history permissions: {err}"))
        })?;
    }

    #[cfg(not(unix))]
    {
        fs::write(&path, content)
            .map_err(|err| AppError::storage(format!("Could not write history file: {err}")))?;
    }

    Ok(())
}

fn read_job_output_file(job_id: &str) -> AppResult<Option<DocumentOutput>> {
    let path = job_output_json_path(job_id)?;
    match fs::read_to_string(&path) {
        Ok(content) => serde_json::from_str(&content)
            .map(Some)
            .map_err(|err| AppError::storage(format!("Could not parse job output JSON: {err}"))),
        Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(err) => Err(AppError::storage(format!(
            "Could not read job output JSON: {err}"
        ))),
    }
}

fn write_job_output_files(job_id: &str, output: &DocumentOutput) -> AppResult<()> {
    let output_dir = job_output_dir(job_id)?;
    fs::create_dir_all(&output_dir).map_err(|err| {
        AppError::storage(format!("Could not create job output directory: {err}"))
    })?;

    let json = serde_json::to_vec_pretty(output)
        .map_err(|err| AppError::storage(format!("Could not serialize job output JSON: {err}")))?;
    write_private_file(&output_dir.join("output.json"), &json, "job output JSON")?;
    write_private_file(
        &output_dir.join("output.md"),
        output.to_markdown().as_bytes(),
        "job output Markdown",
    )?;
    Ok(())
}

fn remove_job_output_files(job_id: &str) -> AppResult<()> {
    let output_dir = job_output_dir(job_id)?;
    match fs::remove_dir_all(&output_dir) {
        Ok(()) => Ok(()),
        Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(err) => Err(AppError::storage(format!(
            "Could not delete job output directory: {err}"
        ))),
    }
}

fn write_private_file(path: &Path, content: &[u8], label: &str) -> AppResult<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|err| {
            AppError::storage(format!("Could not create {label} directory: {err}"))
        })?;
    }

    #[cfg(unix)]
    {
        use std::io::Write;
        use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
        let mut file = fs::OpenOptions::new()
            .create(true)
            .truncate(true)
            .write(true)
            .mode(0o600)
            .open(path)
            .map_err(|err| AppError::storage(format!("Could not open {label}: {err}")))?;
        file.write_all(content)
            .map_err(|err| AppError::storage(format!("Could not write {label}: {err}")))?;
        file.sync_all()
            .map_err(|err| AppError::storage(format!("Could not sync {label}: {err}")))?;
        fs::set_permissions(path, fs::Permissions::from_mode(0o600)).map_err(|err| {
            AppError::storage(format!("Could not set {label} permissions: {err}"))
        })?;
    }

    #[cfg(not(unix))]
    {
        fs::write(path, content)
            .map_err(|err| AppError::storage(format!("Could not write {label}: {err}")))?;
    }

    Ok(())
}

fn app_data_dir() -> AppResult<PathBuf> {
    dirs::home_dir()
        .map(|home| home.join(".summarizer"))
        .ok_or_else(|| AppError::new("settings", "Could not locate the user home directory."))
}

fn settings_path() -> AppResult<PathBuf> {
    Ok(app_data_dir()?.join("settings.json"))
}

fn history_path() -> AppResult<PathBuf> {
    Ok(app_data_dir()?.join("history.json"))
}

fn job_output_dir(job_id: &str) -> AppResult<PathBuf> {
    validate_job_id_segment(job_id)?;
    Ok(app_data_dir()?.join("jobs").join(job_id))
}

fn job_output_json_path(job_id: &str) -> AppResult<PathBuf> {
    Ok(job_output_dir(job_id)?.join("output.json"))
}

fn file_name(path: &Path) -> String {
    path.file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("document")
        .to_string()
}

fn validate_job_id_segment(job_id: &str) -> AppResult<()> {
    let valid = !job_id.is_empty()
        && !job_id.contains('/')
        && !job_id.contains('\\')
        && job_id != "."
        && job_id != ".."
        && !job_id.contains("..");
    if valid {
        Ok(())
    } else {
        Err(AppError::new("invalid_job_id", "Job id is not valid."))
    }
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let settings = load_settings_from_disk();
    let log_state = logs::LogState::init(&settings)
        .unwrap_or_else(|err| panic!("Could not initialize logging: {}", err.message));
    tracing::info!(
        target: "summarizer_desktop::startup",
        "Document Summarizer desktop starting"
    );
    tauri::Builder::default()
        // single-instance must be the first plugin registered. When the app is
        // relaunched (e.g. an agent runs the binary with `--enqueue <file>`),
        // this forwards the new argv to the already-running instance instead of
        // starting a second one, so the job lands in the live queue + History.
        .plugin(tauri_plugin_single_instance::init(|app, argv, _cwd| {
            tracing::info!(
                target: "summarizer_desktop::cli",
                "Received forwarded arguments from a second instance"
            );
            dispatch_cli_enqueue(app, &argv);
        }))
        .plugin(tauri_plugin_dialog::init())
        .manage(DesktopState::new(settings, load_history_from_disk()))
        .manage(log_state)
        .setup(|app| {
            let app_handle = app.handle().clone();
            let state = app.state::<DesktopState>().inner().clone();
            tauri::async_runtime::spawn(async move {
                kick_queue_worker(app_handle, state).await;
            });
            // Handle `--enqueue` passed to the first instance (app launched
            // directly with a job). Second-instance launches go through the
            // single-instance callback above instead.
            let startup_args: Vec<String> = env::args().collect();
            match parse_cli_enqueue(&startup_args) {
                Ok(Some(request)) => {
                    spawn_cli_enqueue(
                        app.handle().clone(),
                        app.state::<DesktopState>().inner().clone(),
                        request,
                    );
                }
                Ok(None) => {}
                Err(err) => tracing::error!(
                    target: "summarizer_desktop::cli",
                    error = %err.message,
                    "Ignoring invalid CLI enqueue arguments at startup"
                ),
            }
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            start_job,
            enqueue_jobs,
            cancel_job,
            history,
            delete_job,
            save_job_markdown,
            save_job_json,
            load_settings,
            save_settings,
            settings_file_path,
            provider_readiness,
            provider_availability,
            logs::logs_get_paths,
            logs::logs_list_files,
            logs::logs_read_file,
            logs::logs_delete_file,
            logs::logs_ring,
            logs::logs_clear_ring,
            logs::logs_set_level,
            logs::logs_export,
            logs::logs_ingest_frontend,
            logs::logs_subscribe,
            logs::logs_unsubscribe
        ])
        .run(tauri::generate_context!())
        .expect("error while running Document Summarizer desktop app");
}

#[cfg(test)]
mod tests {
    use super::{
        build_queued_jobs, cancel_job_in_store, claim_next_queued_in_store, claude_cli_args,
        codex_cli_args, command_available, desktop_default_pipeline_config, grok_cli_args,
        job_output_dir, next_queued_job_index, parse_cli_enqueue, provider_health_url,
        required_providers, validate_queue_runtime_requirements_with_renderer, CliSettings,
        DesktopJobStatus, DesktopSettings, JobCancelEvent, JobStore, RequiredProvider,
    };
    #[cfg(unix)]
    use super::{
        history_path, job_output_json_path, load_history_from_disk, load_settings_from_disk,
        remove_job_output_files, settings_path, write_history_file, write_job_output_files,
        write_settings_file, DesktopJob, DocumentOutput,
    };
    use chrono::Utc;
    use std::{
        ffi::OsString,
        fs,
        path::PathBuf,
        sync::{Mutex, MutexGuard},
    };
    use summarizer_types::{
        CliProvider, PipelineConfig, SummarizerMode, SummarizerProvider, VisionMode,
    };

    fn args(values: &[&str]) -> Vec<String> {
        values.iter().map(|value| value.to_string()).collect()
    }

    #[test]
    fn parse_cli_enqueue_returns_none_without_enqueue_flag() {
        let parsed = parse_cli_enqueue(&args(&["--config-json", "{}"])).unwrap();
        assert!(parsed.is_none());
        assert!(parse_cli_enqueue(&[]).unwrap().is_none());
    }

    #[test]
    fn parse_cli_enqueue_collects_files_and_defaults_config() {
        let parsed = parse_cli_enqueue(&args(&["--enqueue", "/tmp/a.pdf", "-e", "/tmp/b.pptx"]))
            .unwrap()
            .expect("expected an enqueue request");
        assert_eq!(parsed.files, vec!["/tmp/a.pdf", "/tmp/b.pptx"]);
        assert_eq!(parsed.config, desktop_default_pipeline_config());
    }

    #[test]
    fn parse_cli_enqueue_ignores_leading_program_path() {
        // The single-instance plugin may or may not include argv[0]; a leading
        // program-path token must not swallow or hide the --enqueue flag.
        let parsed = parse_cli_enqueue(&args(&[
            "/Applications/Document Summarizer.app/Contents/MacOS/document-summarizer-desktop",
            "--enqueue",
            "/tmp/a.pdf",
        ]))
        .unwrap()
        .expect("expected an enqueue request");
        assert_eq!(parsed.files, vec!["/tmp/a.pdf"]);
    }

    #[test]
    fn parse_cli_enqueue_supports_equals_form_and_config_override() {
        let parsed = parse_cli_enqueue(&args(&[
            "--enqueue=/tmp/a.pdf",
            "--config-json={\"vision_mode\":\"none\",\"summarizer_provider\":\"llama_cpp\"}",
        ]))
        .unwrap()
        .expect("expected an enqueue request");
        assert_eq!(parsed.files, vec!["/tmp/a.pdf"]);
        assert_eq!(parsed.config.vision_mode, VisionMode::None);
        assert_eq!(
            parsed.config.summarizer_provider,
            SummarizerProvider::LlamaCpp
        );
    }

    #[test]
    fn parse_cli_enqueue_errors_on_missing_values() {
        assert!(parse_cli_enqueue(&args(&["--enqueue"])).is_err());
        assert!(parse_cli_enqueue(&args(&["--enqueue", "/tmp/a.pdf", "--config-json"])).is_err());
    }

    #[test]
    fn parse_cli_enqueue_errors_on_invalid_config_json() {
        assert!(parse_cli_enqueue(&args(&[
            "--enqueue",
            "/tmp/a.pdf",
            "--config-json",
            "not-json"
        ]))
        .is_err());
    }

    static HOME_LOCK: Mutex<()> = Mutex::new(());

    struct HomeGuard {
        _guard: MutexGuard<'static, ()>,
        original: Option<OsString>,
        temp_home: PathBuf,
    }

    impl HomeGuard {
        fn new() -> Self {
            let guard = HOME_LOCK.lock().unwrap();
            let original = std::env::var_os("HOME");
            let temp_home =
                std::env::temp_dir().join(format!("summarizer-settings-{}", uuid::Uuid::new_v4()));
            fs::create_dir_all(&temp_home).unwrap();
            std::env::set_var("HOME", &temp_home);
            Self {
                _guard: guard,
                original,
                temp_home,
            }
        }
    }

    impl Drop for HomeGuard {
        fn drop(&mut self) {
            if let Some(original) = &self.original {
                std::env::set_var("HOME", original);
            } else {
                std::env::remove_var("HOME");
            }
            let _ = fs::remove_dir_all(&self.temp_home);
        }
    }

    #[cfg(unix)]
    fn sample_job(job_id: &str, status: DesktopJobStatus) -> DesktopJob {
        let now = Utc::now();
        DesktopJob {
            job_id: job_id.to_string(),
            status,
            file_path: "/tmp/example.pdf".to_string(),
            file_name: "example.pdf".to_string(),
            created_at: now,
            queued_at: if status == DesktopJobStatus::Queued {
                Some(now)
            } else {
                None
            },
            started_at: if status == DesktopJobStatus::Queued {
                None
            } else {
                Some(now)
            },
            completed_at: match status {
                DesktopJobStatus::Queued | DesktopJobStatus::Processing => None,
                DesktopJobStatus::Completed
                | DesktopJobStatus::Failed
                | DesktopJobStatus::Canceled => Some(now),
            },
            duration_ms: match status {
                DesktopJobStatus::Queued | DesktopJobStatus::Processing => None,
                DesktopJobStatus::Completed
                | DesktopJobStatus::Failed
                | DesktopJobStatus::Canceled => Some(42),
            },
            error: None,
            config: if status == DesktopJobStatus::Queued {
                Some(PipelineConfig::default())
            } else {
                None
            },
            output: None,
        }
    }

    #[cfg(unix)]
    fn sample_output() -> DocumentOutput {
        serde_json::from_value(serde_json::json!({
            "document": {
                "document_id": "doc-1",
                "filename": "example.pdf",
                "total_pages": 1,
                "metadata": {}
            },
            "pages": [
                {
                    "chunk_id": "chunk-1",
                    "doc_title": "example.pdf",
                    "page_number": 1,
                    "text": "Example extracted text.",
                    "tables": [],
                    "summary_notes": ["Sample note"],
                    "summary_topics": ["Sample topic"]
                }
            ],
            "metrics": null
        }))
        .unwrap()
    }

    #[test]
    fn selected_codex_summarizer_requires_codex_readiness() {
        let config = PipelineConfig {
            summarizer_provider: SummarizerProvider::Codex,
            ..PipelineConfig::default()
        };

        let providers = required_providers(&config).unwrap();

        assert_eq!(providers, vec![RequiredProvider::Codex]);
    }

    #[test]
    fn selected_grok_summarizer_requires_grok_readiness() {
        let config = PipelineConfig {
            summarizer_provider: SummarizerProvider::Grok,
            ..PipelineConfig::default()
        };

        let providers = required_providers(&config).unwrap();

        assert_eq!(providers, vec![RequiredProvider::Grok]);
    }

    #[test]
    fn extract_only_pipeline_requires_no_ai_provider() {
        let config = PipelineConfig {
            extract_only: true,
            vision_mode: VisionMode::Codex,
            summarizer_provider: SummarizerProvider::Codex,
            ..PipelineConfig::default()
        };

        let providers = required_providers(&config).unwrap();

        assert!(providers.is_empty());
    }

    #[test]
    fn readiness_tracks_vision_and_summarizer_providers_without_duplicates() {
        let config = PipelineConfig {
            vision_mode: VisionMode::Codex,
            summarizer_provider: SummarizerProvider::Codex,
            ..PipelineConfig::default()
        };

        let providers = required_providers(&config).unwrap();

        assert_eq!(providers, vec![RequiredProvider::Codex]);
    }

    #[test]
    fn cli_provider_override_can_select_grok() {
        let config = PipelineConfig {
            vision_mode: VisionMode::Codex,
            vision_cli_provider: Some(CliProvider::Grok),
            summarizer_provider: SummarizerProvider::Codex,
            summarizer_cli_provider: Some(CliProvider::Grok),
            ..PipelineConfig::default()
        };

        let providers = required_providers(&config).unwrap();

        assert_eq!(providers, vec![RequiredProvider::Grok]);
    }

    #[test]
    fn skipped_summarization_does_not_require_summarizer_provider() {
        let config = PipelineConfig {
            run_summarization: true,
            summarizer_mode: SummarizerMode::Skip,
            summarizer_provider: SummarizerProvider::Codex,
            ..PipelineConfig::default()
        };

        let providers = required_providers(&config).unwrap();

        assert!(providers.is_empty());
    }

    #[test]
    fn pptx_vision_queue_requires_slide_renderer() {
        let config = PipelineConfig {
            vision_mode: VisionMode::Codex,
            ..PipelineConfig::default()
        };

        let error = validate_queue_runtime_requirements_with_renderer(
            &[PathBuf::from("/tmp/deck.pptx")],
            &config,
            false,
        )
        .unwrap_err();

        assert_eq!(error.code, "libreoffice_missing");
        assert!(error.message.contains("Skip Slide Screenshots"));
    }

    #[test]
    fn pptx_queue_allows_missing_renderer_when_slide_screenshots_are_skipped() {
        let config = PipelineConfig {
            vision_mode: VisionMode::Codex,
            skip_images: true,
            ..PipelineConfig::default()
        };

        validate_queue_runtime_requirements_with_renderer(
            &[PathBuf::from("/tmp/deck.pptx")],
            &config,
            false,
        )
        .unwrap();
    }

    #[test]
    fn non_pptx_queue_does_not_require_slide_renderer() {
        let config = PipelineConfig {
            vision_mode: VisionMode::Codex,
            ..PipelineConfig::default()
        };

        validate_queue_runtime_requirements_with_renderer(
            &[PathBuf::from("/tmp/document.pdf")],
            &config,
            false,
        )
        .unwrap();
    }

    #[test]
    fn missing_cli_command_is_not_available() {
        assert!(!command_available(
            "document-summarizer-definitely-missing-command"
        ));
    }

    #[test]
    fn command_available_searches_nvm_node_bins() {
        let home = HomeGuard::new();
        let bin = home.temp_home.join(".nvm/versions/node/v99.0.0/bin");
        fs::create_dir_all(&bin).unwrap();
        let fake_cli = if cfg!(windows) {
            "fake-cli.cmd"
        } else {
            "fake-cli"
        };
        fs::write(bin.join(fake_cli), "#!/bin/sh\nexit 0\n").unwrap();

        assert!(command_available("fake-cli"));
    }

    #[test]
    fn provider_health_url_uses_server_root_when_base_url_ends_in_v1() {
        let url = provider_health_url("http://localhost:11440/v1").unwrap();

        assert_eq!(url.as_str(), "http://localhost:11440/health");
    }

    #[test]
    fn cli_settings_default_reasoning_effort_to_medium() {
        assert_eq!(CliSettings::new("codex").reasoning_effort, "medium");
        assert_eq!(CliSettings::new("claude").reasoning_effort, "medium");
        assert_eq!(CliSettings::new("grok").reasoning_effort, "medium");
    }

    #[test]
    fn cli_args_include_provider_defaults_before_custom_args() {
        let mut codex = CliSettings::new("codex");
        codex.reasoning_effort = "high".to_string();
        codex.args = "--search".to_string();

        let mut claude = CliSettings::new("claude");
        claude.reasoning_effort = "xhigh".to_string();
        claude.args = "--print".to_string();

        let mut grok = CliSettings::new("grok");
        grok.reasoning_effort = "high".to_string();
        grok.args = "--model grok-build-0.1".to_string();

        assert_eq!(
            codex_cli_args(&codex),
            "-c model_reasoning_effort=high --search"
        );
        assert_eq!(claude_cli_args(&claude), "--effort xhigh --print");
        assert_eq!(
            grok_cli_args(&grok),
            "--no-auto-update --sandbox read-only --disable-web-search --no-subagents --no-plan --always-approve --tools= --model grok-build-0.1"
        );
    }

    #[test]
    fn settings_without_reasoning_effort_get_medium_defaults() {
        let mut value = serde_json::to_value(DesktopSettings::default()).unwrap();
        value["providers"]["codex"]
            .as_object_mut()
            .unwrap()
            .remove("reasoning_effort");
        value["providers"]["claude"]
            .as_object_mut()
            .unwrap()
            .remove("reasoning_effort");
        value["providers"]["grok"]
            .as_object_mut()
            .unwrap()
            .remove("reasoning_effort");

        let settings: DesktopSettings = serde_json::from_value(value).unwrap();

        assert_eq!(settings.providers.codex.reasoning_effort, "medium");
        assert_eq!(settings.providers.claude.reasoning_effort, "medium");
        assert_eq!(settings.providers.grok.reasoning_effort, "medium");
    }

    #[test]
    fn legacy_settings_without_grok_or_with_gemini_still_load() {
        let mut value = serde_json::to_value(DesktopSettings::default()).unwrap();
        value["providers"].as_object_mut().unwrap().remove("grok");
        value["providers"]["gemini"] = serde_json::json!({
            "base_url": "https://generativelanguage.googleapis.com",
            "api_key": "legacy",
            "vision_model": "gemini-legacy"
        });
        value["provider_visibility"]["vision"]["gemini"] = serde_json::json!(true);

        let settings: DesktopSettings = serde_json::from_value(value).unwrap();

        assert_eq!(settings.providers.grok.executable, "grok");
        assert!(!settings.provider_visibility.vision.grok);
    }

    #[test]
    fn settings_without_provider_visibility_get_codex_defaults() {
        let mut value = serde_json::to_value(DesktopSettings::default()).unwrap();
        value.as_object_mut().unwrap().remove("provider_visibility");

        let settings: DesktopSettings = serde_json::from_value(value).unwrap();

        assert!(settings.provider_visibility.vision.codex);
        assert!(settings.provider_visibility.classifier.codex);
        assert!(settings.provider_visibility.summarizer.codex);
        assert!(!settings.provider_visibility.vision.grok);
        assert!(!settings.provider_visibility.summarizer.grok);
        assert!(!settings.provider_visibility.vision.llama_cpp);
        assert!(!settings.provider_visibility.summarizer.llama_cpp);
    }

    #[test]
    fn settings_without_pipeline_defaults_enable_desktop_vision_by_default() {
        let mut value = serde_json::to_value(DesktopSettings::default()).unwrap();
        value.as_object_mut().unwrap().remove("pipeline_defaults");

        let settings: DesktopSettings = serde_json::from_value(value).unwrap();

        assert_eq!(settings.pipeline_defaults.vision_mode, VisionMode::Codex);
        assert_eq!(
            settings.pipeline_defaults.summarizer_provider,
            SummarizerProvider::Codex
        );
    }

    #[test]
    fn queued_jobs_are_built_in_fifo_order_with_config_snapshots() {
        let config = PipelineConfig {
            extract_only: true,
            ..PipelineConfig::default()
        };
        let paths = vec![
            PathBuf::from("/tmp/first.txt"),
            PathBuf::from("/tmp/second.txt"),
            PathBuf::from("/tmp/third.txt"),
        ];

        let jobs = build_queued_jobs(&paths, &config);

        assert_eq!(jobs.len(), 3);
        assert_eq!(jobs[0].file_name, "first.txt");
        assert_eq!(jobs[1].file_name, "second.txt");
        assert_eq!(jobs[2].file_name, "third.txt");
        assert!(jobs[0].queued_at < jobs[1].queued_at);
        assert!(jobs[1].queued_at < jobs[2].queued_at);
        assert!(jobs[0].config.as_ref().unwrap().extract_only);
    }

    #[test]
    fn queue_claims_next_job_by_fifo_timestamp() {
        let jobs = build_queued_jobs(
            &[
                PathBuf::from("/tmp/first.txt"),
                PathBuf::from("/tmp/second.txt"),
                PathBuf::from("/tmp/third.txt"),
            ],
            &PipelineConfig::default(),
        );
        let mut store =
            JobStore::from_jobs(vec![jobs[2].clone(), jobs[1].clone(), jobs[0].clone()]);

        let claimed = claim_next_queued_in_store(&mut store, Utc::now()).unwrap();

        assert_eq!(claimed.file_name, "first.txt");
        assert_eq!(claimed.status, DesktopJobStatus::Processing);
        assert_eq!(
            store.active_job_id.as_deref(),
            Some(claimed.job_id.as_str())
        );
        assert!(claim_next_queued_in_store(&mut store, Utc::now()).is_none());
    }

    #[test]
    fn queue_starts_next_after_active_job_reaches_terminal_state() {
        let jobs = build_queued_jobs(
            &[
                PathBuf::from("/tmp/first.txt"),
                PathBuf::from("/tmp/second.txt"),
            ],
            &PipelineConfig::default(),
        );
        let mut store = JobStore::from_jobs(jobs);
        let first = claim_next_queued_in_store(&mut store, Utc::now()).unwrap();
        let first_index = store
            .jobs
            .iter()
            .position(|job| job.job_id == first.job_id)
            .unwrap();
        store.jobs[first_index].status = DesktopJobStatus::Completed;
        store.active_job_id = None;

        let second = claim_next_queued_in_store(&mut store, Utc::now()).unwrap();

        assert_eq!(second.file_name, "second.txt");
        assert_eq!(store.active_job_id.as_deref(), Some(second.job_id.as_str()));
    }

    #[test]
    fn queued_job_can_be_canceled_before_start() {
        let jobs = build_queued_jobs(
            &[PathBuf::from("/tmp/queued.txt")],
            &PipelineConfig::default(),
        );
        let mut store = JobStore::from_jobs(jobs);
        let job_id = store.jobs[0].job_id.clone();

        let (job, event) = cancel_job_in_store(&mut store, &job_id, Utc::now()).unwrap();

        assert_eq!(event, JobCancelEvent::Canceled);
        assert_eq!(job.status, DesktopJobStatus::Canceled);
        assert_eq!(next_queued_job_index(&store.jobs), None);
    }

    #[test]
    fn active_cancel_does_not_release_next_job_until_task_finishes() {
        let jobs = build_queued_jobs(
            &[
                PathBuf::from("/tmp/active.txt"),
                PathBuf::from("/tmp/next.txt"),
            ],
            &PipelineConfig::default(),
        );
        let mut store = JobStore::from_jobs(jobs);
        let active = claim_next_queued_in_store(&mut store, Utc::now()).unwrap();

        let (job, event) = cancel_job_in_store(&mut store, &active.job_id, Utc::now()).unwrap();

        assert_eq!(event, JobCancelEvent::Updated);
        assert_eq!(job.status, DesktopJobStatus::Processing);
        assert_eq!(store.active_job_id.as_deref(), Some(active.job_id.as_str()));
        assert!(store.canceled.contains(&active.job_id));
        assert!(claim_next_queued_in_store(&mut store, Utc::now()).is_none());
    }

    #[test]
    #[cfg(unix)]
    fn settings_file_is_created_with_defaults_when_missing() {
        let _home = HomeGuard::new();
        let path = settings_path().unwrap();

        assert!(!path.exists());

        let settings = load_settings_from_disk();
        let content = fs::read_to_string(&path).unwrap();
        let value: serde_json::Value = serde_json::from_str(&content).unwrap();

        assert!(path.exists());
        assert!(settings.provider_visibility.vision.codex);
        assert_eq!(value["appearance"]["theme"], "light");
    }

    #[test]
    #[cfg(unix)]
    fn history_file_is_created_when_missing() {
        let _home = HomeGuard::new();
        let path = history_path().unwrap();

        assert!(!path.exists());

        let jobs = load_history_from_disk();
        let content = fs::read_to_string(&path).unwrap();
        let value: serde_json::Value = serde_json::from_str(&content).unwrap();

        assert!(jobs.is_empty());
        assert!(path.exists());
        assert_eq!(value.as_array().unwrap().len(), 0);
    }

    #[test]
    #[cfg(unix)]
    fn history_file_round_trips_jobs_with_private_permissions() {
        use std::os::unix::fs::PermissionsExt;

        let _home = HomeGuard::new();
        let mut job = sample_job("job-1", DesktopJobStatus::Canceled);
        job.error = Some("Canceled by test.".to_string());

        write_history_file(std::slice::from_ref(&job)).unwrap();

        let loaded = load_history_from_disk();
        let mode = fs::metadata(history_path().unwrap())
            .unwrap()
            .permissions()
            .mode()
            & 0o777;

        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[0].job_id, job.job_id);
        assert_eq!(loaded[0].status, DesktopJobStatus::Canceled);
        assert_eq!(mode, 0o600);
    }

    #[test]
    #[cfg(unix)]
    fn processing_history_jobs_are_canceled_on_load() {
        let _home = HomeGuard::new();
        let mut job = sample_job("job-2", DesktopJobStatus::Processing);
        job.completed_at = None;
        job.duration_ms = None;

        write_history_file(&[job]).unwrap();

        let loaded = load_history_from_disk();

        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[0].status, DesktopJobStatus::Canceled);
        assert_eq!(
            loaded[0].error.as_deref(),
            Some("App closed before processing completed.")
        );
    }

    #[test]
    #[cfg(unix)]
    fn queued_history_jobs_survive_round_trip_with_config() {
        let _home = HomeGuard::new();
        let mut job = sample_job("job-queued", DesktopJobStatus::Queued);
        job.config.as_mut().unwrap().extract_only = true;

        write_history_file(std::slice::from_ref(&job)).unwrap();

        let loaded = load_history_from_disk();

        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[0].status, DesktopJobStatus::Queued);
        assert!(loaded[0].started_at.is_none());
        assert!(loaded[0].config.as_ref().unwrap().extract_only);
    }

    #[test]
    #[cfg(unix)]
    fn completed_history_jobs_rehydrate_output_files() {
        let _home = HomeGuard::new();
        let job = sample_job("job-3", DesktopJobStatus::Completed);
        let output = sample_output();

        write_history_file(std::slice::from_ref(&job)).unwrap();
        write_job_output_files(&job.job_id, &output).unwrap();

        let loaded = load_history_from_disk();
        let history_content = fs::read_to_string(history_path().unwrap()).unwrap();
        let markdown_content = fs::read_to_string(
            job_output_json_path(&job.job_id)
                .unwrap()
                .with_file_name("output.md"),
        )
        .unwrap();

        assert_eq!(loaded.len(), 1);
        assert_eq!(
            loaded[0].output.as_ref().unwrap().document.filename,
            "example.pdf"
        );
        assert!(!history_content.contains("Sample note"));
        assert!(markdown_content.contains("Sample note"));
    }

    #[test]
    #[cfg(unix)]
    fn embedded_history_output_is_migrated_to_output_files() {
        let _home = HomeGuard::new();
        let mut job = sample_job("job-4", DesktopJobStatus::Completed);
        job.output = Some(sample_output());
        let path = history_path().unwrap();
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, serde_json::to_vec_pretty(&[job]).unwrap()).unwrap();

        let loaded = load_history_from_disk();
        let history_content = fs::read_to_string(&path).unwrap();

        assert_eq!(loaded.len(), 1);
        assert!(loaded[0].output.is_some());
        assert!(job_output_json_path("job-4").unwrap().exists());
        assert!(!history_content.contains("Sample note"));
    }

    #[test]
    #[cfg(unix)]
    fn removing_job_output_files_deletes_artifacts() {
        let _home = HomeGuard::new();
        let output = sample_output();
        write_job_output_files("job-5", &output).unwrap();

        assert!(job_output_json_path("job-5").unwrap().exists());

        remove_job_output_files("job-5").unwrap();

        assert!(!job_output_json_path("job-5").unwrap().exists());
    }

    #[test]
    fn job_output_dir_rejects_path_traversal_job_ids() {
        assert!(job_output_dir("../outside").is_err());
        assert!(job_output_dir("nested/job").is_err());
        assert!(job_output_dir("job-6").is_ok());
    }

    #[test]
    #[cfg(unix)]
    fn settings_file_is_written_with_private_permissions() {
        use std::os::unix::fs::PermissionsExt;

        let _home = HomeGuard::new();
        write_settings_file(&DesktopSettings::default()).unwrap();
        let mode = fs::metadata(settings_path().unwrap())
            .unwrap()
            .permissions()
            .mode()
            & 0o777;

        assert_eq!(mode, 0o600);
    }
}
