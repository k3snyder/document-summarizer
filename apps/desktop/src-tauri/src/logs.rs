use crate::{app_data_dir, AppError, AppResult, DesktopSettings};
use chrono::{DateTime, Utc};
use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::{
    collections::{HashMap, VecDeque},
    fs::{self, File},
    io::{Read, Seek, SeekFrom, Write},
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
};
use tauri::{async_runtime::JoinHandle, ipc::Channel, State};
use tokio::sync::broadcast;
use tracing::{Event, Level, Subscriber};
use tracing_subscriber::{
    filter::EnvFilter,
    layer::{Context, SubscriberExt},
    registry::LookupSpan,
    reload, Layer, Registry,
};
use walkdir::WalkDir;
use zip::{write::SimpleFileOptions, CompressionMethod, ZipWriter};

const RING_CAPACITY: usize = 2_000;
const DEFAULT_READ_BYTES: u64 = 1_048_576;
const MAX_READ_BYTES: u64 = 10 * 1_048_576;
const FALLBACK_MAX_FILE_MB: u16 = 50;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct LogPathInfo {
    log_dir: String,
    active_file: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct LogFileMeta {
    name: String,
    path: String,
    size_bytes: u64,
    modified_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum RuntimeSource {
    Desktop,
    Frontend,
    DevService,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum LogLevel {
    Trace,
    Debug,
    Info,
    Warn,
    Error,
}

impl LogLevel {
    fn from_tracing(level: &Level) -> Self {
        match *level {
            Level::TRACE => Self::Trace,
            Level::DEBUG => Self::Debug,
            Level::INFO => Self::Info,
            Level::WARN => Self::Warn,
            Level::ERROR => Self::Error,
        }
    }

    fn parse(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "trace" => Some(Self::Trace),
            "debug" => Some(Self::Debug),
            "info" => Some(Self::Info),
            "warn" | "warning" => Some(Self::Warn),
            "error" => Some(Self::Error),
            _ => None,
        }
    }

    fn as_filter(self) -> &'static str {
        match self {
            Self::Trace => "trace",
            Self::Debug => "debug",
            Self::Info => "info",
            Self::Warn => "warn",
            Self::Error => "error",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct LogEvent {
    seq: u64,
    timestamp: DateTime<Utc>,
    source: RuntimeSource,
    level: LogLevel,
    message: String,
    target: String,
    file: Option<String>,
    line: Option<u32>,
    job_id: Option<String>,
    stage: Option<String>,
    fields: Value,
}

#[derive(Clone)]
pub(crate) struct LogHub {
    inner: Arc<Mutex<LogHubInner>>,
    sender: broadcast::Sender<LogEvent>,
    redactor: Redactor,
    file_sink: Arc<LogFileSink>,
    enabled: Arc<AtomicBool>,
}

struct LogHubInner {
    next_seq: u64,
    next_subscription_id: u64,
    events: VecDeque<LogEvent>,
    subscriptions: HashMap<u64, JoinHandle<()>>,
}

pub(crate) struct LogState {
    hub: LogHub,
    filter_handle: reload::Handle<EnvFilter, Registry>,
    enabled: Arc<AtomicBool>,
    log_dir: PathBuf,
}

impl LogState {
    pub(crate) fn init(settings: &DesktopSettings) -> AppResult<Self> {
        let logging = &settings.logging;
        let log_dir = logs_dir()?;
        fs::create_dir_all(&log_dir)
            .map_err(|err| AppError::storage(format!("Could not create log directory: {err}")))?;
        prune_old_logs(&log_dir, logging.retention_days);

        let enabled = Arc::new(AtomicBool::new(logging.enabled));
        let file_sink = Arc::new(LogFileSink::new(
            log_dir.clone(),
            logging.max_file_mb.max(1) as u64 * 1_048_576,
        )?);
        let hub = LogHub::new(file_sink, enabled.clone(), logging.redact_secrets);
        let filter = filter_for(&logging.level, logging.enabled)?;
        let (filter_layer, filter_handle) = reload::Layer::new(filter);
        let subscriber = Registry::default()
            .with(filter_layer)
            .with(LogHubLayer { hub: hub.clone() });

        let _ = tracing_log::LogTracer::init();
        tracing::subscriber::set_global_default(subscriber).map_err(|err| {
            AppError::new(
                "logging",
                format!("Could not initialize tracing subscriber: {err}"),
            )
        })?;

        Ok(Self {
            hub,
            filter_handle,
            enabled,
            log_dir,
        })
    }

    pub(crate) fn set_enabled(&self, enabled: bool) -> AppResult<()> {
        self.enabled.store(enabled, Ordering::Relaxed);
        Ok(())
    }

    pub(crate) fn set_level(&self, level: &str) -> AppResult<()> {
        self.filter_handle
            .reload(filter_for(level, self.enabled.load(Ordering::Relaxed))?)
            .map_err(|err| AppError::new("logging", format!("Could not update log level: {err}")))
    }
}

#[tauri::command]
pub(crate) fn logs_get_paths() -> AppResult<LogPathInfo> {
    let log_dir = logs_dir()?;
    Ok(LogPathInfo {
        active_file: active_log_file_path(&log_dir)?.display().to_string(),
        log_dir: log_dir.display().to_string(),
    })
}

#[tauri::command]
pub(crate) fn logs_list_files(state: State<'_, LogState>) -> AppResult<Vec<LogFileMeta>> {
    list_log_files(&state.log_dir)
}

#[tauri::command]
pub(crate) fn logs_read_file(
    state: State<'_, LogState>,
    name: String,
    max_bytes: Option<u64>,
    from_end: Option<bool>,
) -> AppResult<Vec<LogEvent>> {
    let path = safe_log_file_path(&state.log_dir, &name)?;
    let max_bytes = max_bytes
        .unwrap_or(DEFAULT_READ_BYTES)
        .clamp(1, MAX_READ_BYTES);
    let content = read_file_window(&path, max_bytes, from_end.unwrap_or(true))?;
    Ok(content
        .lines()
        .filter_map(|line| serde_json::from_str::<LogEvent>(line).ok())
        .collect())
}

#[tauri::command]
pub(crate) fn logs_delete_file(
    state: State<'_, LogState>,
    name: String,
) -> AppResult<Vec<LogFileMeta>> {
    let path = safe_log_file_path(&state.log_dir, &name)?;
    state.hub.file_sink.release_path(&path)?;
    fs::remove_file(&path)
        .map_err(|err| AppError::storage(format!("Could not delete log file: {err}")))?;
    tracing::info!(
        target: "summarizer_desktop::logs",
        file_name = %name,
        "Log file deleted"
    );
    list_log_files(&state.log_dir)
}

#[tauri::command]
pub(crate) fn logs_ring(state: State<'_, LogState>, limit: Option<usize>) -> Vec<LogEvent> {
    state.hub.backfill(limit.unwrap_or(500))
}

#[tauri::command]
pub(crate) fn logs_clear_ring(state: State<'_, LogState>) {
    state.hub.clear_ring();
}

#[tauri::command]
pub(crate) fn logs_set_level(
    state: State<'_, LogState>,
    desktop_state: State<'_, crate::DesktopState>,
    level: String,
) -> AppResult<crate::DesktopSettings> {
    let parsed = LogLevel::parse(&level)
        .ok_or_else(|| AppError::new("logging", format!("Unsupported log level: {level}")))?;
    state.set_level(parsed.as_filter())?;
    let mut settings = crate::current_settings(&desktop_state)?;
    settings.logging.level = parsed.as_filter().to_string();
    crate::write_settings_file(&settings)?;
    let mut guard = desktop_state
        .settings
        .write()
        .map_err(|_| AppError::new("settings", "Settings lock is poisoned"))?;
    *guard = settings.clone();
    tracing::info!(target: "summarizer_desktop::logs", level = parsed.as_filter(), "Log level changed");
    Ok(settings)
}

#[tauri::command]
pub(crate) fn logs_export(state: State<'_, LogState>, output_path: String) -> AppResult<String> {
    let output_path = PathBuf::from(output_path);
    if let Some(parent) = output_path.parent() {
        fs::create_dir_all(parent).map_err(|err| {
            AppError::storage(format!("Could not create log export directory: {err}"))
        })?;
    }

    let file = File::create(&output_path)
        .map_err(|err| AppError::storage(format!("Could not create log export: {err}")))?;
    let mut zip = ZipWriter::new(file);
    let options = SimpleFileOptions::default().compression_method(CompressionMethod::Deflated);

    for entry in WalkDir::new(&state.log_dir)
        .into_iter()
        .filter_map(Result::ok)
    {
        if !entry.file_type().is_file() || !is_log_file(entry.path()) {
            continue;
        }
        let name = entry
            .path()
            .strip_prefix(&state.log_dir)
            .map_err(|err| AppError::storage(format!("Could not package log file: {err}")))?;
        let name = name.to_string_lossy().replace('\\', "/");
        zip.start_file(name, options)
            .map_err(|err| AppError::storage(format!("Could not write log export: {err}")))?;
        let mut source = File::open(entry.path())
            .map_err(|err| AppError::storage(format!("Could not read log file: {err}")))?;
        std::io::copy(&mut source, &mut zip)
            .map_err(|err| AppError::storage(format!("Could not copy log file: {err}")))?;
    }

    zip.finish()
        .map_err(|err| AppError::storage(format!("Could not finish log export: {err}")))?;
    tracing::info!(
        target: "summarizer_desktop::logs",
        output_path = %output_path.display(),
        "Log bundle exported"
    );
    Ok(output_path.display().to_string())
}

#[tauri::command]
pub(crate) fn logs_ingest_frontend(
    state: State<'_, LogState>,
    level: String,
    message: String,
    fields: Option<Value>,
) -> AppResult<()> {
    let level = LogLevel::parse(&level).ok_or_else(|| {
        AppError::new(
            "logging",
            format!("Unsupported frontend log level: {level}"),
        )
    })?;
    state.hub.ingest(LogEvent {
        seq: 0,
        timestamp: Utc::now(),
        source: RuntimeSource::Frontend,
        level,
        message,
        target: "summarizer_frontend".to_string(),
        file: None,
        line: None,
        job_id: None,
        stage: None,
        fields: fields.unwrap_or(Value::Object(Map::new())),
    });
    Ok(())
}

#[tauri::command]
pub(crate) fn logs_subscribe(
    state: State<'_, LogState>,
    channel: Channel<LogEvent>,
    backfill: Option<usize>,
) -> AppResult<u64> {
    for event in state.hub.backfill(backfill.unwrap_or(500)) {
        channel
            .send(event)
            .map_err(|err| AppError::new("logging", format!("Could not send log event: {err}")))?;
    }
    let subscription_id = state.hub.allocate_subscription_id()?;
    let mut receiver = state.hub.subscribe();
    let hub = state.hub.clone();
    let handle = tauri::async_runtime::spawn(async move {
        while let Ok(event) = receiver.recv().await {
            if channel.send(event).is_err() {
                break;
            }
        }
        hub.finish_subscription(subscription_id);
    });
    state.hub.store_subscription(subscription_id, handle)?;
    Ok(subscription_id)
}

#[tauri::command]
pub(crate) fn logs_unsubscribe(state: State<'_, LogState>, subscription_id: u64) -> AppResult<()> {
    state.hub.unsubscribe(subscription_id);
    Ok(())
}

impl LogHub {
    fn new(file_sink: Arc<LogFileSink>, enabled: Arc<AtomicBool>, redact_enabled: bool) -> Self {
        let (sender, _) = broadcast::channel(RING_CAPACITY);
        Self {
            inner: Arc::new(Mutex::new(LogHubInner {
                next_seq: 1,
                next_subscription_id: 1,
                events: VecDeque::with_capacity(RING_CAPACITY),
                subscriptions: HashMap::new(),
            })),
            sender,
            redactor: Redactor::new(redact_enabled),
            file_sink,
            enabled,
        }
    }

    fn ingest(&self, mut event: LogEvent) {
        if !self.enabled.load(Ordering::Relaxed) {
            return;
        }
        self.redactor.redact_event(&mut event);
        {
            let mut inner = match self.inner.lock() {
                Ok(inner) => inner,
                Err(_) => return,
            };
            event.seq = inner.next_seq;
            inner.next_seq = inner.next_seq.saturating_add(1);
            if inner.events.len() == RING_CAPACITY {
                inner.events.pop_front();
            }
            inner.events.push_back(event.clone());
        }
        let _ = self.file_sink.write_event(&event);
        let _ = self.sender.send(event);
    }

    fn backfill(&self, limit: usize) -> Vec<LogEvent> {
        let Ok(inner) = self.inner.lock() else {
            return Vec::new();
        };
        let start = inner.events.len().saturating_sub(limit);
        inner.events.iter().skip(start).cloned().collect()
    }

    fn clear_ring(&self) {
        if let Ok(mut inner) = self.inner.lock() {
            inner.events.clear();
        }
    }

    fn subscribe(&self) -> broadcast::Receiver<LogEvent> {
        self.sender.subscribe()
    }

    fn allocate_subscription_id(&self) -> AppResult<u64> {
        let mut inner = self
            .inner
            .lock()
            .map_err(|_| AppError::new("logging", "Log hub lock is poisoned"))?;
        let id = inner.next_subscription_id;
        inner.next_subscription_id = inner.next_subscription_id.saturating_add(1).max(1);
        Ok(id)
    }

    fn store_subscription(&self, id: u64, handle: JoinHandle<()>) -> AppResult<()> {
        let mut inner = self
            .inner
            .lock()
            .map_err(|_| AppError::new("logging", "Log hub lock is poisoned"))?;
        inner.subscriptions.insert(id, handle);
        Ok(())
    }

    fn unsubscribe(&self, id: u64) {
        if let Ok(mut inner) = self.inner.lock() {
            if let Some(handle) = inner.subscriptions.remove(&id) {
                handle.abort();
            }
        }
    }

    fn finish_subscription(&self, id: u64) {
        if let Ok(mut inner) = self.inner.lock() {
            inner.subscriptions.remove(&id);
        }
    }
}

struct LogHubLayer {
    hub: LogHub,
}

impl<S> Layer<S> for LogHubLayer
where
    S: Subscriber + for<'lookup> LookupSpan<'lookup>,
{
    fn on_event(&self, event: &Event<'_>, _ctx: Context<'_, S>) {
        let metadata = event.metadata();
        let mut visitor = EventVisitor::default();
        event.record(&mut visitor);
        let fields = Value::Object(visitor.fields);
        let job_id = string_field(&fields, "job_id").or_else(|| string_field(&fields, "jobId"));
        let stage = string_field(&fields, "stage");
        self.hub.ingest(LogEvent {
            seq: 0,
            timestamp: Utc::now(),
            source: RuntimeSource::Desktop,
            level: LogLevel::from_tracing(metadata.level()),
            message: visitor.message.unwrap_or_default(),
            target: metadata.target().to_string(),
            file: metadata.file().map(ToString::to_string),
            line: metadata.line(),
            job_id,
            stage,
            fields,
        });
    }
}

#[derive(Default)]
struct EventVisitor {
    message: Option<String>,
    fields: Map<String, Value>,
}

impl tracing::field::Visit for EventVisitor {
    fn record_str(&mut self, field: &tracing::field::Field, value: &str) {
        self.record_value(field.name(), Value::String(value.to_string()));
    }

    fn record_bool(&mut self, field: &tracing::field::Field, value: bool) {
        self.record_value(field.name(), Value::Bool(value));
    }

    fn record_i64(&mut self, field: &tracing::field::Field, value: i64) {
        self.record_value(field.name(), Value::Number(value.into()));
    }

    fn record_u64(&mut self, field: &tracing::field::Field, value: u64) {
        self.record_value(field.name(), Value::Number(value.into()));
    }

    fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
        self.record_value(field.name(), Value::String(format!("{value:?}")));
    }
}

impl EventVisitor {
    fn record_value(&mut self, name: &str, value: Value) {
        if name == "message" {
            self.message = value.as_str().map(ToString::to_string).or_else(|| {
                Some(
                    value
                        .to_string()
                        .trim_matches('"')
                        .replace("\\\"", "\"")
                        .to_string(),
                )
            });
        } else {
            self.fields.insert(name.to_string(), value);
        }
    }
}

#[derive(Clone)]
struct Redactor {
    enabled: bool,
    token_patterns: Arc<Vec<Regex>>,
    key_value_pattern: Arc<Regex>,
}

impl Redactor {
    fn new(enabled: bool) -> Self {
        let patterns = [
            r"(?i)(sk-[A-Za-z0-9_\-]{12,})",
            r"(?i)(xox[baprs]-[A-Za-z0-9\-]{12,})",
            r"(?i)(gh[pousr]_[A-Za-z0-9_]{12,})",
        ]
        .iter()
        .filter_map(|pattern| Regex::new(pattern).ok())
        .collect();
        Self {
            enabled,
            token_patterns: Arc::new(patterns),
            key_value_pattern: Arc::new(
                Regex::new(
                    r"(?i)(api[_-]?key|authorization|bearer|token|secret|password)(\s*[:=]\s*)([^\s,;]+)",
                )
                .expect("static redaction regex must compile"),
            ),
        }
    }

    fn redact_event(&self, event: &mut LogEvent) {
        if !self.enabled {
            return;
        }
        event.message = self.redact_str(&event.message);
        event.target = self.redact_str(&event.target);
        if let Some(file) = &mut event.file {
            *file = self.redact_str(file);
        }
        redact_value(self, &mut event.fields);
    }

    fn redact_str(&self, value: &str) -> String {
        let without_tokens = self
            .token_patterns
            .iter()
            .fold(value.to_string(), |acc, pattern| {
                pattern.replace_all(&acc, "[REDACTED]").to_string()
            });
        self.key_value_pattern
            .replace_all(&without_tokens, "$1$2[REDACTED]")
            .to_string()
    }
}

struct LogFileSink {
    dir: PathBuf,
    max_file_bytes: u64,
    state: Mutex<HashMap<String, LogFileState>>,
}

struct LogFileState {
    path: PathBuf,
    index: u16,
    file: File,
    bytes_written: u64,
}

impl LogFileSink {
    fn new(dir: PathBuf, max_file_bytes: u64) -> AppResult<Self> {
        Ok(Self {
            dir,
            max_file_bytes: if max_file_bytes == 0 {
                FALLBACK_MAX_FILE_MB as u64 * 1_048_576
            } else {
                max_file_bytes
            },
            state: Mutex::new(HashMap::new()),
        })
    }

    fn write_event(&self, event: &LogEvent) -> AppResult<()> {
        let mut line = serde_json::to_vec(event)
            .map_err(|err| AppError::storage(format!("Could not serialize log event: {err}")))?;
        line.push(b'\n');
        let base_name = event_log_base_name(event);
        let mut state_guard = self
            .state
            .lock()
            .map_err(|_| AppError::new("logging", "Log file lock is poisoned"))?;

        let should_open = state_guard
            .get(&base_name)
            .is_none_or(|state| state.bytes_written + line.len() as u64 > self.max_file_bytes);
        if should_open {
            let next_index = match state_guard.get(&base_name) {
                Some(state) => state.index.saturating_add(1),
                None => next_log_index(&self.dir, &base_name)?,
            };
            state_guard.insert(
                base_name.clone(),
                open_log_file(&self.dir, &base_name, next_index)?,
            );
        }

        let state = state_guard
            .get_mut(&base_name)
            .ok_or_else(|| AppError::new("logging", "Log file was not opened"))?;
        state
            .file
            .write_all(&line)
            .map_err(|err| AppError::storage(format!("Could not write log file: {err}")))?;
        state.bytes_written += line.len() as u64;
        Ok(())
    }

    fn release_path(&self, path: &Path) -> AppResult<()> {
        let mut state_guard = self
            .state
            .lock()
            .map_err(|_| AppError::new("logging", "Log file lock is poisoned"))?;
        state_guard.retain(|_, state| state.path.as_path() != path);
        Ok(())
    }
}

fn redact_value(redactor: &Redactor, value: &mut Value) {
    match value {
        Value::String(inner) => *inner = redactor.redact_str(inner),
        Value::Array(items) => {
            for item in items {
                redact_value(redactor, item);
            }
        }
        Value::Object(map) => {
            for (key, item) in map.iter_mut() {
                if is_sensitive_log_key(key) {
                    *item = Value::String("[REDACTED]".to_string());
                } else {
                    redact_value(redactor, item);
                }
            }
        }
        Value::Null | Value::Bool(_) | Value::Number(_) => {}
    }
}

fn is_sensitive_log_key(key: &str) -> bool {
    let lowered = key.to_ascii_lowercase();
    let parts: Vec<&str> = lowered
        .split(|ch: char| !ch.is_ascii_alphanumeric())
        .filter(|part| !part.is_empty())
        .collect();

    matches!(
        lowered.as_str(),
        "authorization" | "password" | "secret" | "api_key" | "apikey"
    ) || parts.iter().any(|part| {
        matches!(
            *part,
            "authorization" | "password" | "secret" | "apikey" | "api_key" | "key" | "token"
        )
    })
}

fn string_field(fields: &Value, key: &str) -> Option<String> {
    fields
        .get(key)
        .and_then(Value::as_str)
        .map(ToString::to_string)
}

fn filter_for(level: &str, enabled: bool) -> AppResult<EnvFilter> {
    if !enabled {
        return EnvFilter::try_new("off")
            .map_err(|err| AppError::new("logging", format!("Invalid off filter: {err}")));
    }
    let parsed = LogLevel::parse(level)
        .ok_or_else(|| AppError::new("logging", format!("Unsupported log level: {level}")))?;
    EnvFilter::try_new(parsed.as_filter())
        .map_err(|err| AppError::new("logging", format!("Invalid log filter: {err}")))
}

fn logs_dir() -> AppResult<PathBuf> {
    Ok(app_data_dir()?.join("logs"))
}

fn active_log_file_path(log_dir: &Path) -> AppResult<PathBuf> {
    Ok(log_dir.join(log_file_name("desktop", 0)))
}

fn event_log_base_name(event: &LogEvent) -> String {
    event
        .job_id
        .as_deref()
        .map(str::trim)
        .filter(|job_id| !job_id.is_empty())
        .map(|job_id| format!("job-{}", sanitize_log_component(job_id)))
        .filter(|base_name| base_name != "job-")
        .unwrap_or_else(|| "desktop".to_string())
}

fn sanitize_log_component(value: &str) -> String {
    let mut sanitized = String::with_capacity(value.len().min(120));
    let mut previous_was_separator = false;

    for ch in value.chars().take(120) {
        if ch.is_ascii_alphanumeric() || ch == '-' || ch == '_' {
            sanitized.push(ch);
            previous_was_separator = false;
        } else if !previous_was_separator {
            sanitized.push('-');
            previous_was_separator = true;
        }
    }

    sanitized.trim_matches('-').to_string()
}

fn log_file_name(base_name: &str, index: u16) -> String {
    if index == 0 {
        format!("{base_name}.jsonl")
    } else {
        format!("{base_name}.{index}.jsonl")
    }
}

fn next_log_index(dir: &Path, base_name: &str) -> AppResult<u16> {
    let mut index = 0;
    while dir.join(log_file_name(base_name, index)).exists() {
        if index == u16::MAX {
            return Err(AppError::storage(format!(
                "Too many rotated log files for {base_name}"
            )));
        }
        index = index.saturating_add(1);
    }
    Ok(index)
}

fn open_log_file(dir: &Path, base_name: &str, index: u16) -> AppResult<LogFileState> {
    fs::create_dir_all(dir)
        .map_err(|err| AppError::storage(format!("Could not create log directory: {err}")))?;
    let path = dir.join(log_file_name(base_name, index));
    #[cfg(unix)]
    let file = {
        use std::os::unix::fs::OpenOptionsExt;
        fs::OpenOptions::new()
            .create(true)
            .append(true)
            .mode(0o600)
            .open(&path)
    };
    #[cfg(not(unix))]
    let file = fs::OpenOptions::new().create(true).append(true).open(&path);
    let file = file.map_err(|err| AppError::storage(format!("Could not open log file: {err}")))?;
    let bytes_written = file
        .metadata()
        .map(|metadata| metadata.len())
        .unwrap_or_default();
    Ok(LogFileState {
        path,
        index,
        file,
        bytes_written,
    })
}

fn list_log_files(dir: &Path) -> AppResult<Vec<LogFileMeta>> {
    fs::create_dir_all(dir)
        .map_err(|err| AppError::storage(format!("Could not create log directory: {err}")))?;
    let mut files = Vec::new();
    for entry in fs::read_dir(dir)
        .map_err(|err| AppError::storage(format!("Could not read log directory: {err}")))?
    {
        let entry =
            entry.map_err(|err| AppError::storage(format!("Could not read log entry: {err}")))?;
        let path = entry.path();
        if !is_log_file(&path) {
            continue;
        }
        let metadata = entry
            .metadata()
            .map_err(|err| AppError::storage(format!("Could not read log metadata: {err}")))?;
        let modified_at = metadata.modified().ok().map(DateTime::<Utc>::from);
        files.push(LogFileMeta {
            name: entry.file_name().to_string_lossy().to_string(),
            path: path.display().to_string(),
            size_bytes: metadata.len(),
            modified_at,
        });
    }
    files.sort_by(|left, right| right.modified_at.cmp(&left.modified_at));
    Ok(files)
}

fn safe_log_file_path(dir: &Path, name: &str) -> AppResult<PathBuf> {
    if name.is_empty()
        || name.contains('/')
        || name.contains('\\')
        || name == "."
        || name == ".."
        || !is_valid_log_file_name(name)
    {
        return Err(AppError::new("logging", "Invalid log file name."));
    }
    let path = dir.join(name);
    if !path.is_file() {
        return Err(AppError::new(
            "not_found",
            format!("Log file not found: {name}"),
        ));
    }
    Ok(path)
}

fn is_log_file(path: &Path) -> bool {
    path.file_name()
        .and_then(|name| name.to_str())
        .is_some_and(is_valid_log_file_name)
}

fn is_valid_log_file_name(name: &str) -> bool {
    if !name.ends_with(".jsonl") {
        return false;
    }

    let stem = name.trim_end_matches(".jsonl");
    if stem == "desktop" || stem.starts_with("desktop-") {
        return true;
    }
    if let Some(rest) = stem.strip_prefix("desktop.") {
        return rest.parse::<u16>().is_ok();
    }
    if let Some(rest) = stem.strip_prefix("job-") {
        let (job_part, rotation_part) = rest.rsplit_once('.').unwrap_or((rest, ""));
        return !job_part.is_empty()
            && job_part
                .chars()
                .all(|ch| ch.is_ascii_alphanumeric() || ch == '-' || ch == '_')
            && (rotation_part.is_empty() || rotation_part.parse::<u16>().is_ok());
    }
    false
}

fn read_file_window(path: &Path, max_bytes: u64, from_end: bool) -> AppResult<String> {
    let mut file =
        File::open(path).map_err(|err| AppError::storage(format!("Could not open log: {err}")))?;
    let len = file
        .metadata()
        .map_err(|err| AppError::storage(format!("Could not read log metadata: {err}")))?
        .len();
    if from_end && len > max_bytes {
        file.seek(SeekFrom::Start(len - max_bytes))
            .map_err(|err| AppError::storage(format!("Could not seek log: {err}")))?;
    }
    let mut content = String::new();
    file.take(max_bytes)
        .read_to_string(&mut content)
        .map_err(|err| AppError::storage(format!("Could not read log: {err}")))?;
    if from_end && len > max_bytes {
        if let Some(index) = content.find('\n') {
            content = content[index + 1..].to_string();
        }
    }
    Ok(content)
}

fn prune_old_logs(dir: &Path, retention_days: u16) {
    if retention_days == 0 {
        return;
    }
    let cutoff = std::time::SystemTime::now()
        .checked_sub(std::time::Duration::from_secs(
            retention_days as u64 * 24 * 60 * 60,
        ))
        .unwrap_or(std::time::SystemTime::UNIX_EPOCH);
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.filter_map(Result::ok) {
        let path = entry.path();
        if !is_log_file(&path) {
            continue;
        }
        let Ok(metadata) = entry.metadata() else {
            continue;
        };
        if metadata.modified().is_ok_and(|modified| modified < cutoff) {
            let _ = fs::remove_file(path);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        event_log_base_name, is_valid_log_file_name, redact_value, LogEvent, LogFileSink, LogHub,
        LogLevel, Redactor, RuntimeSource,
    };
    use chrono::Utc;
    use serde_json::json;
    use std::{
        fs,
        sync::{atomic::AtomicBool, Arc},
        time::SystemTime,
    };

    #[test]
    fn redacts_secret_fields_and_common_token_shapes() {
        let redactor = Redactor::new(true);
        let mut event = LogEvent {
            seq: 0,
            timestamp: Utc::now(),
            source: RuntimeSource::Desktop,
            level: LogLevel::Info,
            message: "authorization: bearer sk-abcdefghijklmnop".to_string(),
            target: "test".to_string(),
            file: None,
            line: None,
            job_id: None,
            stage: None,
            fields: json!({
                "api_key": "sk-secretsecretsecret",
                "nested": { "token": "ghp_abcdefghijklmnop" },
                "tokens": 1234,
                "total_tokens": 5678,
                "classification_tokens": 9,
                "safe": "visible"
            }),
        };

        redactor.redact_event(&mut event);
        redact_value(&redactor, &mut event.fields);

        assert!(event.message.contains("[REDACTED]"));
        assert_eq!(event.fields["api_key"], "[REDACTED]");
        assert_eq!(event.fields["nested"]["token"], "[REDACTED]");
        assert_eq!(event.fields["tokens"], 1234);
        assert_eq!(event.fields["total_tokens"], 5678);
        assert_eq!(event.fields["classification_tokens"], 9);
        assert_eq!(event.fields["safe"], "visible");
    }

    #[test]
    fn job_events_use_job_log_base_name() {
        let event = LogEvent {
            seq: 0,
            timestamp: Utc::now(),
            source: RuntimeSource::Desktop,
            level: LogLevel::Info,
            message: "Job started".to_string(),
            target: "test".to_string(),
            file: None,
            line: None,
            job_id: Some("ffdeefb1-d854-4c62-b917-47572640b694".to_string()),
            stage: None,
            fields: json!({}),
        };

        assert_eq!(
            event_log_base_name(&event),
            "job-ffdeefb1-d854-4c62-b917-47572640b694"
        );
    }

    #[test]
    fn log_file_validation_accepts_job_desktop_and_legacy_files() {
        assert!(is_valid_log_file_name("job-ffdeefb1-d854.jsonl"));
        assert!(is_valid_log_file_name("job-ffdeefb1-d854.1.jsonl"));
        assert!(is_valid_log_file_name("desktop.jsonl"));
        assert!(is_valid_log_file_name("desktop.1.jsonl"));
        assert!(is_valid_log_file_name("desktop-2026-06-05.jsonl"));
        assert!(is_valid_log_file_name("desktop-2026-06-05.3.jsonl"));

        assert!(!is_valid_log_file_name("job-.jsonl"));
        assert!(!is_valid_log_file_name("job-abc.not-a-number.jsonl"));
        assert!(!is_valid_log_file_name("../job-abc.jsonl"));
        assert!(!is_valid_log_file_name("notes.txt"));
    }

    #[test]
    fn file_sink_writes_job_events_to_job_files() {
        let unique = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .expect("system clock should be after unix epoch")
            .as_nanos();
        let dir = std::env::temp_dir().join(format!("summarizer-log-test-{unique}"));
        fs::create_dir_all(&dir).expect("test log dir should be created");

        let sink = LogFileSink::new(dir.clone(), 1_048_576).expect("sink should initialize");
        let mut event = LogEvent {
            seq: 1,
            timestamp: Utc::now(),
            source: RuntimeSource::Desktop,
            level: LogLevel::Info,
            message: "Job started".to_string(),
            target: "test".to_string(),
            file: None,
            line: None,
            job_id: Some("ffdeefb1-d854-4c62-b917-47572640b694".to_string()),
            stage: None,
            fields: json!({ "job_id": "ffdeefb1-d854-4c62-b917-47572640b694" }),
        };

        sink.write_event(&event).expect("job event should write");
        event.job_id = None;
        event.fields = json!({});
        sink.write_event(&event)
            .expect("desktop event should write");

        assert!(dir
            .join("job-ffdeefb1-d854-4c62-b917-47572640b694.jsonl")
            .is_file());
        assert!(dir.join("desktop.jsonl").is_file());

        fs::remove_dir_all(dir).expect("test log dir should be removed");
    }

    #[tokio::test]
    async fn log_hub_unsubscribe_removes_registered_forwarder() {
        let unique = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .expect("system clock should be after unix epoch")
            .as_nanos();
        let dir = std::env::temp_dir().join(format!("summarizer-log-sub-test-{unique}"));
        fs::create_dir_all(&dir).expect("test log dir should be created");

        let sink = Arc::new(LogFileSink::new(dir.clone(), 1_048_576).expect("sink should init"));
        let hub = LogHub::new(sink, Arc::new(AtomicBool::new(true)), true);
        let id = hub.allocate_subscription_id().unwrap();
        let handle = tauri::async_runtime::spawn(async {
            std::future::pending::<()>().await;
        });

        hub.store_subscription(id, handle).unwrap();
        assert_eq!(hub.inner.lock().unwrap().subscriptions.len(), 1);

        hub.unsubscribe(id);

        assert!(hub.inner.lock().unwrap().subscriptions.is_empty());
        fs::remove_dir_all(dir).expect("test log dir should be removed");
    }
}
