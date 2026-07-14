use async_trait::async_trait;
use base64::{engine::general_purpose, Engine as _};
use image::codecs::jpeg::JpegEncoder;
use image::imageops::FilterType;
use image::{DynamicImage, GenericImageView, ImageReader};
use reqwest::{Client, Response, StatusCode};
use serde::de::DeserializeOwned;
use serde_json::json;
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::io::Cursor;
use std::path::Path;
use std::process::Stdio;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use summarizer_cli_util::{
    cli_command_context, configure_isolated_grok_command, create_isolated_grok_home,
    parse_codex_jsonl_output, parse_grok_json, resolve_cli_executable, run_cli_command_with_retry,
    RetryPolicy,
};
use summarizer_types::PipelineError;
use tokio::process::Command;

const CLASSIFIER_PROMPT: &str = include_str!("../../../prompts/vision-classifier.txt");
const EXTRACT_PROMPT: &str = include_str!("../../../prompts/vision-extract.txt");
const CLASSIFIER_MAX_TOKENS: u16 = 16;
const EXTRACTION_MAX_TOKENS: u16 = 2048;
const DEFAULT_MAX_INLINE_IMAGE_BYTES: usize = 45_000_000;
const MIN_INLINE_IMAGE_BYTES: usize = 100_000;
const MAX_IMAGE_PIXELS: u64 = 16_000_000;
const HTTP_CONNECT_TIMEOUT_SECONDS: u64 = 10;
const HTTP_REQUEST_TIMEOUT_SECONDS: u64 = 300;
const MAX_IMAGE_NORMALIZATION_CACHE_ENTRIES: usize = 8;
const MAX_PROVIDER_ERROR_BODY_BYTES: usize = 2048;
const INLINE_IMAGE_RESIZE_STEPS: &[(u32, u8)] = &[
    (1600, 82),
    (1400, 78),
    (1200, 74),
    (1000, 70),
    (800, 65),
    (640, 60),
    (512, 55),
    (384, 50),
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CliProviderKind {
    Generic,
    Codex,
    Grok,
    Copilot,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VisionPage {
    pub page_number: usize,
    pub chunk_id: String,
    pub image_base64: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct InlineImagePayload {
    mime_type: &'static str,
    base64: String,
}

#[derive(Debug, Clone, Default)]
struct ImageNormalizationCache {
    entries: Arc<Mutex<HashMap<String, InlineImagePayload>>>,
}

impl ImageNormalizationCache {
    async fn get_or_normalize(
        &self,
        image_base64: &str,
        max_bytes: usize,
    ) -> Result<InlineImagePayload, PipelineError> {
        let key = image_cache_key(image_base64);
        if let Some(payload) = self
            .entries
            .lock()
            .map_err(|_| PipelineError::Vision("image normalization cache poisoned".to_string()))?
            .get(&key)
            .cloned()
        {
            return Ok(payload);
        }

        let image_base64 = image_base64.to_string();
        let payload =
            tokio::task::spawn_blocking(move || normalize_inline_image(&image_base64, max_bytes))
                .await
                .map_err(|err| {
                    PipelineError::Vision(format!("image normalization task failed: {err}"))
                })??;
        let mut entries = self
            .entries
            .lock()
            .map_err(|_| PipelineError::Vision("image normalization cache poisoned".to_string()))?;
        if entries.len() >= MAX_IMAGE_NORMALIZATION_CACHE_ENTRIES {
            entries.clear();
        }
        entries.insert(key, payload.clone());
        Ok(payload)
    }

    fn remove(&self, image_base64: &str) -> Result<(), PipelineError> {
        let key = image_cache_key(image_base64);
        self.entries
            .lock()
            .map_err(|_| PipelineError::Vision("image normalization cache poisoned".to_string()))?
            .remove(&key);
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClassificationResult {
    pub page_number: usize,
    pub chunk_id: String,
    pub has_graphics: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VisionExtractionResult {
    pub page_number: usize,
    pub chunk_id: String,
    pub image_text: Option<String>,
}

#[async_trait]
pub trait VisionProvider: Send + Sync {
    async fn classify(&self, page: &VisionPage) -> Result<ClassificationResult, PipelineError>;
    async fn extract(&self, page: &VisionPage) -> Result<VisionExtractionResult, PipelineError>;

    fn release_page(&self, _page: &VisionPage) -> Result<(), PipelineError> {
        Ok(())
    }

    fn reported_model(&self) -> Option<String> {
        None
    }
}

#[derive(Debug, Clone)]
pub struct NoopVisionProvider;

#[async_trait]
impl VisionProvider for NoopVisionProvider {
    async fn classify(&self, page: &VisionPage) -> Result<ClassificationResult, PipelineError> {
        Ok(ClassificationResult {
            page_number: page.page_number,
            chunk_id: page.chunk_id.clone(),
            has_graphics: false,
        })
    }

    async fn extract(&self, page: &VisionPage) -> Result<VisionExtractionResult, PipelineError> {
        Ok(VisionExtractionResult {
            page_number: page.page_number,
            chunk_id: page.chunk_id.clone(),
            image_text: None,
        })
    }
}

#[derive(Debug, Clone)]
pub struct OpenAiCompatibleVisionProvider {
    client: Client,
    base_url: String,
    api_key: Option<String>,
    model: String,
    disable_thinking: bool,
    image_cache: ImageNormalizationCache,
}

impl OpenAiCompatibleVisionProvider {
    pub fn new(
        base_url: impl Into<String>,
        api_key: Option<String>,
        model: impl Into<String>,
    ) -> Self {
        Self {
            client: provider_http_client(Duration::from_secs(HTTP_REQUEST_TIMEOUT_SECONDS)),
            base_url: base_url.into().trim_end_matches('/').to_string(),
            api_key,
            model: model.into(),
            disable_thinking: false,
            image_cache: ImageNormalizationCache::default(),
        }
    }

    pub fn with_llama_cpp_options(mut self) -> Self {
        self.disable_thinking = true;
        self
    }

    pub fn with_http_timeout(mut self, timeout: Duration) -> Self {
        self.client = provider_http_client(timeout);
        self
    }

    async fn chat(
        &self,
        prompt: &str,
        image_base64: &str,
        max_tokens: u16,
    ) -> Result<String, PipelineError> {
        let url = format!("{}/chat/completions", self.base_url);
        let image_payload = self
            .image_cache
            .get_or_normalize(image_base64, max_inline_image_bytes())
            .await?;
        let mut body = json!({
            "model": self.model,
            "messages": [{
                "role": "user",
                "content": [
                    {"type": "text", "text": prompt},
                    {"type": "image_url", "image_url": {"url": format!("data:{};base64,{}", image_payload.mime_type, image_payload.base64)}}
                ]
            }],
            "temperature": 0,
            "max_tokens": max_tokens
        });
        if self.disable_thinking {
            body["chat_template_kwargs"] = json!({"enable_thinking": false});
        }
        let mut request = self.client.post(url).json(&body);

        if let Some(api_key) = &self.api_key {
            request = request.bearer_auth(api_key);
        }

        let response = request.send().await.map_err(vision_http_error)?;
        let value: serde_json::Value =
            vision_json_response(response, "OpenAI-compatible vision provider").await?;

        value["choices"][0]["message"]["content"]
            .as_str()
            .map(|content| content.trim().to_string())
            .ok_or_else(|| PipelineError::Vision("missing chat completion content".to_string()))
    }
}

impl OpenAiCompatibleVisionProvider {
    fn release_cached_page(&self, page: &VisionPage) -> Result<(), PipelineError> {
        self.image_cache.remove(&page.image_base64)
    }
}

#[async_trait]
impl VisionProvider for OpenAiCompatibleVisionProvider {
    async fn classify(&self, page: &VisionPage) -> Result<ClassificationResult, PipelineError> {
        let content = self
            .chat(CLASSIFIER_PROMPT, &page.image_base64, CLASSIFIER_MAX_TOKENS)
            .await?;
        Ok(ClassificationResult {
            page_number: page.page_number,
            chunk_id: page.chunk_id.clone(),
            has_graphics: content.to_ascii_uppercase().contains("YES"),
        })
    }

    async fn extract(&self, page: &VisionPage) -> Result<VisionExtractionResult, PipelineError> {
        let content = self
            .chat(EXTRACT_PROMPT, &page.image_base64, EXTRACTION_MAX_TOKENS)
            .await?;
        Ok(VisionExtractionResult {
            page_number: page.page_number,
            chunk_id: page.chunk_id.clone(),
            image_text: Some(content),
        })
    }

    fn release_page(&self, page: &VisionPage) -> Result<(), PipelineError> {
        self.release_cached_page(page)
    }
}

#[derive(Debug, Clone)]
pub struct GeminiVisionProvider {
    client: Client,
    base_url: String,
    api_key: Option<String>,
    model: String,
    image_cache: ImageNormalizationCache,
}

impl GeminiVisionProvider {
    pub fn new(
        base_url: impl Into<String>,
        api_key: Option<String>,
        model: impl Into<String>,
    ) -> Self {
        Self {
            client: provider_http_client(Duration::from_secs(HTTP_REQUEST_TIMEOUT_SECONDS)),
            base_url: base_url.into().trim_end_matches('/').to_string(),
            api_key,
            model: model.into(),
            image_cache: ImageNormalizationCache::default(),
        }
    }

    pub fn with_http_timeout(mut self, timeout: Duration) -> Self {
        self.client = provider_http_client(timeout);
        self
    }

    async fn generate(&self, prompt: &str, image_base64: &str) -> Result<String, PipelineError> {
        let url = format!(
            "{}/v1beta/models/{}:generateContent",
            self.base_url, self.model
        );

        let image_payload = self
            .image_cache
            .get_or_normalize(image_base64, max_inline_image_bytes())
            .await?;
        let mut request = self.client.post(url)
            .json(&json!({
                "contents": [{
                    "role": "user",
                    "parts": [
                        {"text": prompt},
                        {"inline_data": {"mime_type": image_payload.mime_type, "data": image_payload.base64}}
                    ]
                }],
                "generationConfig": {"temperature": 0.0}
            }));
        if let Some(api_key) = &self.api_key {
            request = request.header("x-goog-api-key", api_key);
        }

        let response = request.send().await.map_err(vision_http_error)?;
        let value: serde_json::Value =
            vision_json_response(response, "Gemini vision provider").await?;

        value["candidates"][0]["content"]["parts"]
            .as_array()
            .and_then(|parts| parts.iter().find_map(|part| part["text"].as_str()))
            .map(|content| content.trim().to_string())
            .ok_or_else(|| PipelineError::Vision("missing Gemini response text".to_string()))
    }
}

#[async_trait]
impl VisionProvider for GeminiVisionProvider {
    async fn classify(&self, page: &VisionPage) -> Result<ClassificationResult, PipelineError> {
        let content = self.generate(CLASSIFIER_PROMPT, &page.image_base64).await?;
        Ok(ClassificationResult {
            page_number: page.page_number,
            chunk_id: page.chunk_id.clone(),
            has_graphics: content.to_ascii_uppercase().contains("YES"),
        })
    }

    async fn extract(&self, page: &VisionPage) -> Result<VisionExtractionResult, PipelineError> {
        let content = self.generate(EXTRACT_PROMPT, &page.image_base64).await?;
        Ok(VisionExtractionResult {
            page_number: page.page_number,
            chunk_id: page.chunk_id.clone(),
            image_text: Some(content),
        })
    }

    fn release_page(&self, page: &VisionPage) -> Result<(), PipelineError> {
        self.image_cache.remove(&page.image_base64)
    }
}

#[derive(Debug, Clone)]
pub struct CliVisionProvider {
    executable: String,
    args: Vec<String>,
    timeout_seconds: u64,
    retries: u32,
    kind: CliProviderKind,
    reported_model: Arc<Mutex<Option<String>>>,
}

impl CliVisionProvider {
    pub fn new(executable: impl Into<String>) -> Self {
        Self {
            executable: executable.into(),
            args: Vec::new(),
            timeout_seconds: 600,
            retries: 3,
            kind: CliProviderKind::Generic,
            reported_model: Arc::new(Mutex::new(None)),
        }
    }

    pub fn codex(executable: impl Into<String>) -> Self {
        Self {
            executable: executable.into(),
            args: Vec::new(),
            timeout_seconds: 600,
            retries: 3,
            kind: CliProviderKind::Codex,
            reported_model: Arc::new(Mutex::new(None)),
        }
    }

    pub fn grok(executable: impl Into<String>) -> Self {
        Self {
            executable: executable.into(),
            args: Vec::new(),
            timeout_seconds: 600,
            retries: 3,
            kind: CliProviderKind::Grok,
            reported_model: Arc::new(Mutex::new(None)),
        }
    }

    pub fn copilot(executable: impl Into<String>) -> Self {
        Self {
            executable: executable.into(),
            args: Vec::new(),
            timeout_seconds: 600,
            retries: 3,
            kind: CliProviderKind::Copilot,
            reported_model: Arc::new(Mutex::new(None)),
        }
    }

    pub fn with_args(mut self, args: impl IntoIterator<Item = impl Into<String>>) -> Self {
        self.args = args.into_iter().map(Into::into).collect();
        self
    }

    pub fn with_timeout_seconds(mut self, timeout_seconds: u64) -> Self {
        self.timeout_seconds = timeout_seconds;
        self
    }

    pub fn with_retries(mut self, retries: u32) -> Self {
        self.retries = retries;
        self
    }

    fn retry_policy(&self) -> RetryPolicy {
        RetryPolicy::new(self.retries)
    }

    async fn execute(&self, prompt: &str, page: &VisionPage) -> Result<String, PipelineError> {
        if self.kind == CliProviderKind::Codex {
            return self.execute_codex_image(prompt, page).await;
        }
        if self.kind == CliProviderKind::Grok {
            return self.execute_grok_image(prompt, page).await;
        }
        if self.kind == CliProviderKind::Copilot {
            return self.execute_copilot_image(prompt, page).await;
        }

        let context = cli_command_context(
            "CLI vision provider",
            &self.executable,
            &self.args,
            self.timeout_seconds,
        );
        let request = format!(
            "{prompt}\n\nPage: {}\nChunk: {}\nImage base64:\n{}",
            page.page_number, page.chunk_id, page.image_base64
        );
        let executable = resolved_cli_executable_value(&self.executable);
        let make_command = || {
            let mut command = Command::new(&executable);
            command
                .args(&self.args)
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped());
            command
        };
        let output = run_cli_command_with_retry(
            make_command,
            &request,
            &context,
            self.timeout_seconds,
            "CLI vision provider",
            self.retry_policy(),
        )
        .await
        .map_err(PipelineError::Vision)?;
        Ok(output.stdout)
    }

    async fn execute_codex_image(
        &self,
        prompt: &str,
        page: &VisionPage,
    ) -> Result<String, PipelineError> {
        let temp_dir = tempfile::tempdir().map_err(|err| {
            PipelineError::Vision(format!("could not create Codex temp dir: {err}"))
        })?;
        let image_path = temp_dir
            .path()
            .join(format!("page_{}.png", page.page_number));
        let image_bytes = decode_base64_image(&page.image_base64)?;
        tokio::fs::write(&image_path, image_bytes)
            .await
            .map_err(|err| PipelineError::Vision(format!("could not write Codex image: {err}")))?;

        let request = format!(
            "Analyze the attached image for page {} / chunk {}.\n\n{prompt}\n\nReturn ONLY the requested result with no preamble.",
            page.page_number, page.chunk_id
        );

        self.execute_codex_exec(&request, temp_dir.path(), Some(&image_path))
            .await
    }

    async fn execute_codex_exec(
        &self,
        prompt: &str,
        working_dir: &Path,
        image_path: Option<&Path>,
    ) -> Result<String, PipelineError> {
        let mut args = vec![
            "exec".to_string(),
            "-C".to_string(),
            working_dir.display().to_string(),
            "-s".to_string(),
            "read-only".to_string(),
            "--skip-git-repo-check".to_string(),
            "--json".to_string(),
        ];
        args.extend(self.args.clone());
        if let Some(image_path) = image_path {
            args.push("--image".to_string());
            args.push(image_path.display().to_string());
        }
        args.push("-".to_string());
        let context = cli_command_context(
            "Codex CLI vision provider",
            &self.executable,
            &args,
            self.timeout_seconds,
        );
        let executable = resolved_cli_executable_value(&self.executable);
        let make_command = || {
            let mut command = Command::new(&executable);
            command
                .args(&args)
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped());
            command
        };

        let output = run_cli_command_with_retry(
            make_command,
            prompt,
            &context,
            self.timeout_seconds,
            "CLI vision provider",
            self.retry_policy(),
        )
        .await
        .map_err(PipelineError::Vision)?;
        let parsed = parse_codex_jsonl_output(&output.stdout);
        if let Some(model) = parsed.model {
            if let Ok(mut reported_model) = self.reported_model.lock() {
                *reported_model = Some(model);
            }
        }
        if parsed.content.trim().is_empty() {
            return Err(PipelineError::Vision(format!(
                "Codex CLI returned no assistant message; {context}; stdout={}; stderr={}",
                output.stdout.trim(),
                output.stderr.trim()
            )));
        }
        Ok(parsed.content)
    }

    async fn execute_grok_image(
        &self,
        prompt: &str,
        page: &VisionPage,
    ) -> Result<String, PipelineError> {
        let temp_dir = tempfile::tempdir().map_err(|err| {
            PipelineError::Vision(format!("could not create Grok temp dir: {err}"))
        })?;
        let image_path = temp_dir
            .path()
            .join(format!("page_{}.png", page.page_number));
        let image_bytes = decode_base64_image(&page.image_base64)?;
        tokio::fs::write(&image_path, image_bytes)
            .await
            .map_err(|err| PipelineError::Vision(format!("could not write Grok image: {err}")))?;

        let request = format!(
            "Analyze the image file @{} for page {} / chunk {}.\n\n{prompt}\n\nReturn ONLY the requested result with no preamble.",
            image_path
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or("page.png"),
            page.page_number,
            page.chunk_id
        );

        self.execute_grok_headless(&request, temp_dir.path()).await
    }

    async fn execute_grok_headless(
        &self,
        prompt: &str,
        working_dir: &Path,
    ) -> Result<String, PipelineError> {
        let prompt_path = working_dir.join("prompt.txt");
        tokio::fs::write(&prompt_path, prompt)
            .await
            .map_err(|err| PipelineError::Vision(format!("could not write Grok prompt: {err}")))?;
        let grok_home = create_isolated_grok_home(working_dir)
            .await
            .map_err(PipelineError::Vision)?;
        let mut args = self.args.clone();
        args.extend([
            "--cwd".to_string(),
            working_dir.display().to_string(),
            "--output-format".to_string(),
            "json".to_string(),
            "--prompt-file".to_string(),
            prompt_path.display().to_string(),
        ]);
        let context = cli_command_context(
            "Grok CLI vision provider",
            &self.executable,
            &args,
            self.timeout_seconds,
        );
        let executable = resolved_cli_executable_value(&self.executable);
        let make_command = || {
            let mut command = Command::new(&executable);
            configure_isolated_grok_command(&mut command, &grok_home);
            command
                .args(&args)
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped());
            command
        };

        let output = run_cli_command_with_retry(
            make_command,
            "",
            &context,
            self.timeout_seconds,
            "CLI vision provider",
            self.retry_policy(),
        )
        .await
        .map_err(PipelineError::Vision)?;
        let content = parse_grok_json(&output.stdout);
        if content.trim().is_empty() {
            return Err(PipelineError::Vision(format!(
                "Grok CLI returned no assistant text; {context}; stdout={}; stderr={}",
                output.stdout.trim(),
                output.stderr.trim()
            )));
        }
        Ok(content)
    }

    async fn execute_copilot_image(
        &self,
        prompt: &str,
        page: &VisionPage,
    ) -> Result<String, PipelineError> {
        let temp_dir = tempfile::tempdir().map_err(|err| {
            PipelineError::Vision(format!("could not create Copilot temp dir: {err}"))
        })?;
        let image_path = temp_dir
            .path()
            .join(format!("page_{}.png", page.page_number));
        let image_bytes = decode_base64_image(&page.image_base64)?;
        tokio::fs::write(&image_path, image_bytes)
            .await
            .map_err(|err| {
                PipelineError::Vision(format!("could not write Copilot image: {err}"))
            })?;

        let request = format!(
            "Read and analyze the image file {} in the current working directory for page {} / chunk {}.\n\n{prompt}\n\nReturn ONLY the requested result with no preamble.",
            image_path
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or("page.png"),
            page.page_number,
            page.chunk_id
        );

        self.execute_copilot_prompt(&request, Some(temp_dir.path()))
            .await
    }

    async fn execute_copilot_prompt(
        &self,
        prompt: &str,
        working_dir: Option<&Path>,
    ) -> Result<String, PipelineError> {
        let mut args = vec![
            "-p".to_string(),
            prompt.to_string(),
            "--allow-all-tools".to_string(),
            "--no-color".to_string(),
            "-s".to_string(),
        ];
        if let Some(dir) = working_dir {
            args.push("-C".to_string());
            args.push(dir.display().to_string());
        }
        args.extend(self.args.clone());
        let context = cli_command_context(
            "Copilot CLI vision provider",
            &self.executable,
            &args,
            self.timeout_seconds,
        );
        let executable = resolved_cli_executable_value(&self.executable);
        let make_command = || {
            let mut command = Command::new(&executable);
            command
                .args(&args)
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped());
            command
        };

        let output = run_cli_command_with_retry(
            make_command,
            "",
            &context,
            self.timeout_seconds,
            "CLI vision provider",
            self.retry_policy(),
        )
        .await
        .map_err(PipelineError::Vision)?;
        let content = output.stdout.trim().to_string();
        if content.is_empty() {
            return Err(PipelineError::Vision(format!(
                "Copilot CLI returned empty output; {context}; stderr={}",
                output.stderr.trim()
            )));
        }
        Ok(content)
    }
}

fn resolved_cli_executable_value(executable: &str) -> String {
    resolve_cli_executable(executable)
        .map(|path| path.display().to_string())
        .unwrap_or_else(|| executable.trim().to_string())
}

#[async_trait]
impl VisionProvider for CliVisionProvider {
    async fn classify(&self, page: &VisionPage) -> Result<ClassificationResult, PipelineError> {
        if self.kind == CliProviderKind::Codex || self.kind == CliProviderKind::Copilot {
            return Ok(ClassificationResult {
                page_number: page.page_number,
                chunk_id: page.chunk_id.clone(),
                has_graphics: true,
            });
        }

        let content = self.execute(CLASSIFIER_PROMPT, page).await?;
        Ok(ClassificationResult {
            page_number: page.page_number,
            chunk_id: page.chunk_id.clone(),
            has_graphics: content.to_ascii_uppercase().contains("YES"),
        })
    }

    async fn extract(&self, page: &VisionPage) -> Result<VisionExtractionResult, PipelineError> {
        let content = self.execute(EXTRACT_PROMPT, page).await?;
        Ok(VisionExtractionResult {
            page_number: page.page_number,
            chunk_id: page.chunk_id.clone(),
            image_text: if content.is_empty() {
                None
            } else {
                Some(content)
            },
        })
    }

    fn reported_model(&self) -> Option<String> {
        self.reported_model.lock().ok()?.clone()
    }
}

fn decode_base64_image(value: &str) -> Result<Vec<u8>, PipelineError> {
    let payload = value
        .strip_prefix("data:")
        .and_then(|data| data.split_once(',').map(|(_, payload)| payload))
        .unwrap_or(value);
    general_purpose::STANDARD
        .decode(payload)
        .map_err(|err| PipelineError::Vision(format!("invalid base64 image for CLI vision: {err}")))
}

fn normalize_inline_image(
    image_base64: &str,
    max_bytes: usize,
) -> Result<InlineImagePayload, PipelineError> {
    let max_bytes = max_bytes.max(MIN_INLINE_IMAGE_BYTES);
    let original_bytes = decode_base64_image(image_base64).map_err(|err| {
        PipelineError::Vision(format!("could not decode image for vision request: {err}"))
    })?;
    check_image_dimensions(&original_bytes)?;
    if base64_fits(image_base64, max_bytes) {
        return Ok(InlineImagePayload {
            mime_type: "image/png",
            base64: image_base64.to_string(),
        });
    }

    let image = image::load_from_memory(&original_bytes).map_err(|err| {
        PipelineError::Vision(format!(
            "could not decode oversized image for vision request: {err}"
        ))
    })?;

    let mut best: Option<Vec<u8>> = None;
    for (max_dimension, quality) in INLINE_IMAGE_RESIZE_STEPS {
        let resized = resize_to_max_dimension(&image, *max_dimension);
        let jpeg = encode_jpeg(&resized, *quality)?;
        if jpeg.len() <= max_bytes {
            return Ok(InlineImagePayload {
                mime_type: "image/jpeg",
                base64: general_purpose::STANDARD.encode(jpeg),
            });
        }
        if best
            .as_ref()
            .is_none_or(|current| jpeg.len() < current.len())
        {
            best = Some(jpeg);
        }
    }

    let Some(jpeg) = best else {
        return Ok(InlineImagePayload {
            mime_type: "image/png",
            base64: image_base64.to_string(),
        });
    };
    if jpeg.len() > max_bytes {
        return Err(PipelineError::Vision(format!(
            "could not compress image below {max_bytes} bytes for vision request; smallest compressed image was {} bytes",
            jpeg.len()
        )));
    }

    Ok(InlineImagePayload {
        mime_type: "image/jpeg",
        base64: general_purpose::STANDARD.encode(jpeg),
    })
}

fn base64_fits(value: &str, max_bytes: usize) -> bool {
    let max_base64_chars = max_bytes.saturating_mul(4).div_ceil(3) + 4;
    value.len() <= max_base64_chars
}

fn image_cache_key(image_base64: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(image_base64.as_bytes());
    format!("{:x}", hasher.finalize())
}

fn check_image_dimensions(bytes: &[u8]) -> Result<(), PipelineError> {
    let reader = ImageReader::new(Cursor::new(bytes))
        .with_guessed_format()
        .map_err(|err| {
            PipelineError::Vision(format!("could not inspect image dimensions: {err}"))
        })?;
    let (width, height) = reader.into_dimensions().map_err(|err| {
        PipelineError::Vision(format!("could not inspect image dimensions: {err}"))
    })?;
    let pixels = u64::from(width) * u64::from(height);
    if pixels > MAX_IMAGE_PIXELS {
        return Err(PipelineError::Vision(format!(
            "image dimensions {width}x{height} exceed limit of {MAX_IMAGE_PIXELS} pixels"
        )));
    }
    Ok(())
}

fn resize_to_max_dimension(image: &DynamicImage, max_dimension: u32) -> DynamicImage {
    let (width, height) = image.dimensions();
    if width <= max_dimension && height <= max_dimension {
        image.clone()
    } else {
        image.resize(max_dimension, max_dimension, FilterType::Triangle)
    }
}

fn encode_jpeg(image: &DynamicImage, quality: u8) -> Result<Vec<u8>, PipelineError> {
    let rgb = image.to_rgb8();
    let (width, height) = rgb.dimensions();
    let mut bytes = Cursor::new(Vec::new());
    JpegEncoder::new_with_quality(&mut bytes, quality)
        .encode(&rgb, width, height, image::ExtendedColorType::Rgb8)
        .map_err(|err| {
            PipelineError::Vision(format!(
                "could not compress image for vision request: {err}"
            ))
        })?;
    Ok(bytes.into_inner())
}

fn max_inline_image_bytes() -> usize {
    std::env::var("VISION_MAX_INLINE_IMAGE_BYTES")
        .ok()
        .and_then(|value| value.parse::<usize>().ok())
        .filter(|value| *value >= MIN_INLINE_IMAGE_BYTES)
        .unwrap_or(DEFAULT_MAX_INLINE_IMAGE_BYTES)
}

fn provider_http_client(timeout: Duration) -> Client {
    Client::builder()
        .connect_timeout(Duration::from_secs(HTTP_CONNECT_TIMEOUT_SECONDS))
        .timeout(timeout)
        .build()
        .expect("provider HTTP client configuration should be valid")
}

fn vision_http_error(err: reqwest::Error) -> PipelineError {
    if err.is_timeout() {
        PipelineError::Vision(format!("vision provider request timed out: {err}"))
    } else {
        PipelineError::Vision(err.to_string())
    }
}

async fn vision_json_response<T: DeserializeOwned>(
    response: Response,
    provider_name: &str,
) -> Result<T, PipelineError> {
    let status = response.status();
    if !status.is_success() {
        let body = read_vision_error_body(response).await?;
        return Err(PipelineError::Vision(status_error_message(
            provider_name,
            status,
            &body,
        )));
    }

    response
        .json()
        .await
        .map_err(|err| PipelineError::Vision(err.to_string()))
}

async fn read_vision_error_body(mut response: Response) -> Result<String, PipelineError> {
    let mut body = Vec::new();
    let mut truncated = false;

    while let Some(chunk) = response.chunk().await.map_err(vision_http_error)? {
        let remaining = MAX_PROVIDER_ERROR_BODY_BYTES.saturating_sub(body.len());
        if chunk.len() > remaining {
            body.extend_from_slice(&chunk[..remaining]);
            truncated = true;
            break;
        }

        body.extend_from_slice(&chunk);
        if body.len() == MAX_PROVIDER_ERROR_BODY_BYTES {
            truncated = true;
            break;
        }
    }

    let mut text = String::from_utf8_lossy(&body).trim().to_string();
    if truncated {
        text.push_str("... [truncated]");
    }
    Ok(text)
}

fn status_error_message(provider_name: &str, status: StatusCode, body: &str) -> String {
    if body.is_empty() {
        format!("{provider_name} returned HTTP {status}")
    } else {
        format!("{provider_name} returned HTTP {status}: {body}")
    }
}

#[cfg(test)]
mod tests {
    use super::{
        cli_command_context, normalize_inline_image, ImageNormalizationCache, InlineImagePayload,
        MAX_IMAGE_NORMALIZATION_CACHE_ENTRIES,
    };
    use base64::{engine::general_purpose, Engine as _};
    use image::{DynamicImage, ImageBuffer, ImageFormat, Rgb};
    use std::io::Cursor;
    use summarizer_cli_util::unavailable_output;

    #[test]
    fn cli_error_context_names_command_args_timeout_and_streams() {
        let args = vec!["--model".to_string(), "vision-model".to_string()];
        let context = cli_command_context("CLI vision provider", "/usr/bin/example", &args, 42);
        assert!(context.contains("executable=/usr/bin/example"));
        assert!(context.contains("--model"));
        assert!(context.contains("vision-model"));
        assert!(context.contains("timeout_seconds=42"));

        let output = unavailable_output("process did not start");
        assert!(output.contains("stdout="));
        assert!(output.contains("stderr="));
    }

    #[test]
    fn small_inline_images_are_left_unchanged() {
        let image = png_base64(2, 2);
        let payload = normalize_inline_image(&image, 100_000).unwrap();
        assert_eq!(
            payload,
            InlineImagePayload {
                mime_type: "image/png",
                base64: image,
            }
        );
    }

    #[test]
    fn oversized_inline_images_are_downscaled_and_reencoded() {
        let original = patterned_png_base64(900, 900);

        let payload = normalize_inline_image(&original, 100_000).unwrap();

        assert_eq!(payload.mime_type, "image/jpeg");
        assert!(payload.base64.len() < original.len());
        let decoded = general_purpose::STANDARD.decode(payload.base64).unwrap();
        assert!(image::load_from_memory(&decoded).is_ok());
    }

    #[test]
    fn oversized_image_dimensions_are_rejected_before_decode() {
        let image = png_base64(4097, 4097);
        let error = normalize_inline_image(&image, 60_000_000).unwrap_err();

        assert!(error.to_string().contains("exceed limit"));
    }

    #[tokio::test]
    async fn image_normalization_cache_returns_cached_payload() {
        let cache = ImageNormalizationCache::default();
        let image = patterned_png_base64(900, 900);

        let first = cache.get_or_normalize(&image, 100_000).await.unwrap();
        let second = cache.get_or_normalize(&image, 100_000).await.unwrap();

        assert_eq!(first, second);
        assert_eq!(cache.entries.lock().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn image_normalization_cache_removes_page_payload() {
        let cache = ImageNormalizationCache::default();
        let image = patterned_png_base64(900, 900);

        cache.get_or_normalize(&image, 100_000).await.unwrap();
        assert_eq!(cache.entries.lock().unwrap().len(), 1);

        cache.remove(&image).unwrap();

        assert_eq!(cache.entries.lock().unwrap().len(), 0);
    }

    #[tokio::test]
    async fn image_normalization_cache_is_bounded() {
        let cache = ImageNormalizationCache::default();

        for index in 0..(MAX_IMAGE_NORMALIZATION_CACHE_ENTRIES + 2) {
            let image = png_base64(2 + index as u32, 2);
            cache.get_or_normalize(&image, 100_000).await.unwrap();
        }

        assert!(cache.entries.lock().unwrap().len() <= MAX_IMAGE_NORMALIZATION_CACHE_ENTRIES);
    }

    fn png_base64(width: u32, height: u32) -> String {
        let image = DynamicImage::ImageRgb8(ImageBuffer::from_pixel(width, height, Rgb([0, 0, 0])));
        let mut png = Cursor::new(Vec::new());
        image.write_to(&mut png, ImageFormat::Png).unwrap();
        general_purpose::STANDARD.encode(png.into_inner())
    }

    fn patterned_png_base64(width: u32, height: u32) -> String {
        let image = DynamicImage::ImageRgb8(ImageBuffer::from_fn(width, height, |x, y| {
            Rgb([
                ((x * 37 + y * 17) % 256) as u8,
                ((x * 11 + y * 29) % 256) as u8,
                ((x * 23 + y * 7) % 256) as u8,
            ])
        }));
        let mut png = Cursor::new(Vec::new());
        image.write_to(&mut png, ImageFormat::Png).unwrap();
        general_purpose::STANDARD.encode(png.into_inner())
    }
}
