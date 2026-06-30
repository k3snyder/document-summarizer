use std::path::Path;
use std::sync::Arc;
use std::time::Instant;
use summarizer_extraction::{ExtractionProgress, Extractor};
use summarizer_summarization::{
    CliSummarizer, OpenAiCompatibleSummarizer, SummarizationBudgetExhaustReason,
    SummarizationOptions, Summarizer,
};
use summarizer_types::{
    CliProvider, PageOutput, PipelineConfig, PipelineError, PipelineMetrics, PipelineMetricsConfig,
    PipelineStages, StageMetrics, SummarizerMode, SummarizerProvider, VisionMode,
};
use summarizer_vision::{
    CliVisionProvider, GeminiVisionProvider, OpenAiCompatibleVisionProvider, VisionPage,
    VisionProvider,
};

pub use summarizer_extraction::configure_pdfium_library_path;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PipelineProgressStage {
    Extraction,
    Vision,
    Summarization,
}

impl PipelineProgressStage {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Extraction => "extraction",
            Self::Vision => "vision",
            Self::Summarization => "summarization",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Extraction => "Extraction",
            Self::Vision => "Vision",
            Self::Summarization => "Summarization",
        }
    }
}

#[derive(Debug, Clone)]
pub struct PipelineProgress {
    pub stage: PipelineProgressStage,
    pub stage_index: usize,
    pub total_stages: usize,
    pub page_number: Option<usize>,
    pub total_pages: Option<usize>,
    pub progress: u8,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HttpProviderConfig {
    pub base_url: String,
    pub api_key: Option<String>,
    pub model: String,
    pub model_2: String,
    pub model_3: String,
    pub vision_model: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LlamaCppProviderConfig {
    pub base_url: String,
    pub vision_base_url: String,
    pub api_key: Option<String>,
    pub model: String,
    pub model_2: String,
    pub model_3: String,
    pub vision_model: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OllamaProviderConfig {
    pub openai_base_url: String,
    pub api_key: Option<String>,
    pub model: String,
    pub model_2: String,
    pub model_3: String,
    pub vision_model: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GeminiProviderConfig {
    pub base_url: String,
    pub api_key: Option<String>,
    pub vision_model: String,
}

/// Default number of total attempts for a CLI provider call (timeouts, crashes,
/// and spawn failures are retried with exponential backoff up to this many
/// tries). Overridable per provider via `*_CLI_RETRIES` env vars.
pub const DEFAULT_CLI_RETRIES: u32 = 3;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CliRuntimeConfig {
    pub executable: String,
    pub args: Vec<String>,
    pub timeout_seconds: u64,
    pub retries: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PipelineProviderConfig {
    pub openai: HttpProviderConfig,
    pub llama_cpp: LlamaCppProviderConfig,
    pub ollama: OllamaProviderConfig,
    pub gemini: GeminiProviderConfig,
    pub codex: CliRuntimeConfig,
    pub claude: CliRuntimeConfig,
    pub grok: CliRuntimeConfig,
    pub copilot: CliRuntimeConfig,
}

impl PipelineProviderConfig {
    pub fn from_env() -> Self {
        let llama_model = env_or("LLAMA_CPP_MODEL", "model.gguf");
        let openai_model = env_or("OPENAI_MODEL", "gpt-4.1-mini");
        let ollama_model = env_or("OLLAMA_MODEL", "gemma4:12b-it-qat");
        Self {
            openai: HttpProviderConfig {
                base_url: env_or("OPENAI_BASE_URL", "https://api.openai.com/v1"),
                api_key: optional_env("OPENAI_API_KEY"),
                model: openai_model.clone(),
                model_2: env_or("OPENAI_MODEL_2", openai_model.clone()),
                model_3: env_or("OPENAI_MODEL_3", openai_model),
                vision_model: env_or("OPENAI_VISION_MODEL", "gpt-4.1-mini"),
            },
            llama_cpp: LlamaCppProviderConfig {
                base_url: env_or("LLAMA_CPP_BASE_URL", "http://localhost:11440/v1"),
                vision_base_url: env_or(
                    "LLAMA_CPP_VISION_BASE_URL",
                    env_or("LLAMA_CPP_BASE_URL", "http://localhost:11440/v1"),
                ),
                api_key: optional_env("LLAMA_CPP_API_KEY"),
                model: llama_model.clone(),
                model_2: env_or("LLAMA_CPP_MODEL_2", llama_model.clone()),
                model_3: env_or("LLAMA_CPP_MODEL_3", llama_model),
                vision_model: env_or("LLAMA_CPP_VISION_MODEL", "model.gguf"),
            },
            ollama: OllamaProviderConfig {
                openai_base_url: env_or("OLLAMA_OPENAI_BASE_URL", "http://localhost:11434/v1"),
                api_key: optional_env("OLLAMA_API_KEY"),
                model: ollama_model.clone(),
                model_2: env_or("OLLAMA_MODEL_2", ollama_model.clone()),
                model_3: env_or("OLLAMA_MODEL_3", ollama_model),
                vision_model: env_or("VISION_MODEL", "llava"),
            },
            gemini: GeminiProviderConfig {
                base_url: env_or(
                    "GEMINI_BASE_URL",
                    "https://generativelanguage.googleapis.com",
                ),
                api_key: optional_env("GEMINI_API_KEY"),
                vision_model: env_or("GEMINI_VISION_MODEL", "gemini-2.5-flash"),
            },
            codex: CliRuntimeConfig {
                executable: env_or("CODEX_CLI_BIN", "codex"),
                args: env_args("CODEX_CLI_ARGS"),
                timeout_seconds: env_u64("CODEX_CLI_TIMEOUT", 600),
                retries: env_u32("CODEX_CLI_RETRIES", DEFAULT_CLI_RETRIES),
            },
            claude: CliRuntimeConfig {
                executable: env_or("CLAUDE_CLI_BIN", "claude"),
                args: env_args("CLAUDE_CLI_ARGS"),
                timeout_seconds: env_u64("CLAUDE_CLI_TIMEOUT", 600),
                retries: env_u32("CLAUDE_CLI_RETRIES", DEFAULT_CLI_RETRIES),
            },
            grok: CliRuntimeConfig {
                executable: env_or("GROK_CLI_BIN", "grok"),
                args: env_args("GROK_CLI_ARGS"),
                timeout_seconds: env_u64("GROK_CLI_TIMEOUT", 600),
                retries: env_u32("GROK_CLI_RETRIES", DEFAULT_CLI_RETRIES),
            },
            copilot: CliRuntimeConfig {
                executable: env_or("COPILOT_CLI_BIN", "copilot"),
                args: env_args("COPILOT_CLI_ARGS"),
                timeout_seconds: env_u64("COPILOT_CLI_TIMEOUT", 600),
                retries: env_u32("COPILOT_CLI_RETRIES", DEFAULT_CLI_RETRIES),
            },
        }
    }
}

impl Default for PipelineProviderConfig {
    fn default() -> Self {
        Self::from_env()
    }
}

#[derive(Debug, Clone)]
pub struct Pipeline {
    extractor: Extractor,
    provider_config: PipelineProviderConfig,
}

impl Pipeline {
    pub fn new() -> Self {
        Self {
            extractor: Extractor::new(),
            provider_config: PipelineProviderConfig::from_env(),
        }
    }

    pub fn with_provider_config(provider_config: PipelineProviderConfig) -> Self {
        Self {
            extractor: Extractor::new(),
            provider_config,
        }
    }

    pub async fn run_path(
        &self,
        job_id: &str,
        path: &Path,
        config: &PipelineConfig,
    ) -> Result<summarizer_types::DocumentOutput, PipelineError> {
        self.run_path_with_progress(job_id, path, config, |_| {})
            .await
    }

    pub async fn run_path_with_progress<F>(
        &self,
        job_id: &str,
        path: &Path,
        config: &PipelineConfig,
        progress: F,
    ) -> Result<summarizer_types::DocumentOutput, PipelineError>
    where
        F: Fn(PipelineProgress) + Send + Sync + 'static,
    {
        let stages = progress_stages(config);
        let progress: Arc<dyn Fn(PipelineProgress) + Send + Sync> = Arc::new(progress);
        let started = Instant::now();
        let extraction_started = Instant::now();
        tracing::info!(
            target: "summarizer_pipeline",
            job_id,
            file_path = %path.display(),
            extract_only = config.extract_only,
            skip_tables = config.skip_tables,
            skip_images = config.skip_images,
            text_only = config.text_only,
            pdf_image_dpi = %serde_name(config.pdf_image_dpi),
            vision_mode = %serde_name(config.vision_mode),
            vision_classifier_provider = ?resolved_vision_classifier_provider(config),
            vision_extractor_provider = ?resolved_vision_extractor_provider(config),
            summarizer_provider = %serde_name(resolve_summarizer_provider(config)),
            summarizer_mode = %serde_name(config.summarizer_mode),
            summarizer_detailed_extraction = config.summarizer_detailed_extraction,
            summarizer_insight_mode = config.summarizer_insight_mode,
            "Pipeline started"
        );
        emit_progress(
            progress.as_ref(),
            progress_payload(
                &stages,
                PipelineProgressStage::Extraction,
                None,
                None,
                0,
                1,
                "Extracting document content.",
            ),
        );
        let extraction_progress = extraction_progress_callback(&stages, Arc::clone(&progress));
        let mut output = self
            .extractor
            .extract_path_with_progress(format!("doc_{job_id}"), path, config, extraction_progress)
            .await?;
        let extraction_duration = extraction_started.elapsed();
        tracing::info!(
            target: "summarizer_pipeline",
            job_id,
            stage = "extraction",
            duration_ms = extraction_duration.as_millis() as u64,
            pages = output.pages.len(),
            tokens = content_tokens(&output.pages),
            "Extraction completed"
        );
        for (index, page) in output.pages.iter().enumerate() {
            tracing::info!(
                target: "summarizer_pipeline",
                job_id,
                stage = "extraction",
                page_number = page.page_number.unwrap_or(index + 1),
                total_pages = output.pages.len(),
                chunk_id = %page.chunk_id,
                text_chars = page.text.chars().count(),
                text_tokens = page_content_tokens(page),
                table_count = page.tables.len(),
                table_rows = page_table_rows(page),
                embedded_images = page.embedded_images.len(),
                has_page_image = page_has_image(page),
                image_base64_chars = page_image_base64_chars(page),
                "Page extracted"
            );
        }
        emit_progress(
            progress.as_ref(),
            progress_payload(
                &stages,
                PipelineProgressStage::Extraction,
                None,
                Some(output.pages.len()),
                1,
                1,
                format!(
                    "Extracted {} {}.",
                    output.pages.len(),
                    if output.pages.len() == 1 {
                        "page or chunk"
                    } else {
                        "pages or chunks"
                    }
                ),
            ),
        );

        let vision_metrics = run_vision_stage(
            &mut output.pages,
            config,
            &self.provider_config,
            &stages,
            progress.as_ref(),
            job_id,
        )
        .await?;

        let mut summarization_metrics = StageMetrics::empty();
        if config.run_summarization
            && config.summarizer_mode != SummarizerMode::Skip
            && !config.extract_only
        {
            emit_progress(
                progress.as_ref(),
                progress_payload(
                    &stages,
                    PipelineProgressStage::Summarization,
                    None,
                    Some(output.pages.len()),
                    0,
                    output.pages.len().max(1),
                    "Starting summarization.",
                ),
            );
            let summarizer = build_summarizer(config, &self.provider_config)?;
            let summarization_options = SummarizationOptions {
                mode: config.summarizer_mode,
                detailed_extraction: config.summarizer_detailed_extraction,
                insight_mode: config.summarizer_insight_mode,
            };
            let summarization_started = Instant::now();
            tracing::info!(
                target: "summarizer_pipeline",
                job_id,
                stage = "summarization",
                pages = output.pages.len(),
                provider = %serde_name(resolve_summarizer_provider(config)),
                mode = %serde_name(config.summarizer_mode),
                "Summarization started"
            );
            let mut summarized_pages = Vec::with_capacity(output.pages.len());
            let mut total_tokens = 0;
            let mut total_attempts = 0;
            let mut relevancies = Vec::new();
            let total_pages = output.pages.len();
            for (index, page) in output.pages.iter().enumerate() {
                let page_started = Instant::now();
                let page_context_tokens = page_content_tokens(page) + page_vision_tokens(page);
                let page_number = page.page_number.unwrap_or(index + 1);
                tracing::info!(
                    target: "summarizer_pipeline",
                    job_id,
                    stage = "summarization",
                    page_number,
                    total_pages,
                    chunk_id = %page.chunk_id,
                    context_tokens = page_context_tokens,
                    text_chars = page.text.chars().count(),
                    vision_text_chars = page_vision_text_chars(page),
                    table_count = page.tables.len(),
                    "Page summarization started"
                );
                emit_progress(
                    progress.as_ref(),
                    progress_payload(
                        &stages,
                        PipelineProgressStage::Summarization,
                        Some(page_number),
                        Some(total_pages),
                        index,
                        total_pages.max(1),
                        format!("Summarizing page {page_number} of {total_pages}."),
                    ),
                );
                let result = match summarizer.summarize_page(page, summarization_options).await {
                    Ok(result) => result,
                    // Degrade-and-continue: one page's summarizer failure must
                    // not discard the summaries already produced for the rest
                    // of the document.
                    Err(err) => {
                        tracing::warn!(
                            target: "summarizer_pipeline",
                            job_id,
                            stage = "summarization",
                            page_number,
                            total_pages,
                            chunk_id = %page.chunk_id,
                            duration_ms = page_started.elapsed().as_millis() as u64,
                            error = %err,
                            "Page summarization failed after retries; continuing without a summary"
                        );
                        summarized_pages.push(page.clone());
                        emit_progress(
                            progress.as_ref(),
                            progress_payload(
                                &stages,
                                PipelineProgressStage::Summarization,
                                Some(page_number),
                                Some(total_pages),
                                index + 1,
                                total_pages.max(1),
                                format!("Summarized page {page_number} of {total_pages}."),
                            ),
                        );
                        continue;
                    }
                };
                let budget_exhausted = result.budget_exhausted.map(summary_budget_reason);
                tracing::info!(
                    target: "summarizer_pipeline",
                    job_id,
                    stage = "summarization",
                    page_number,
                    total_pages,
                    chunk_id = %page.chunk_id,
                    duration_ms = page_started.elapsed().as_millis() as u64,
                    attempts = result.attempts_used,
                    tokens = result.tokens,
                    effective_tokens = if result.tokens == 0 {
                        page_context_tokens + page_summary_tokens(&result.page)
                    } else {
                        result.tokens
                    },
                    summary_notes = summary_notes_count(&result.page),
                    summary_topics = summary_topics_count(&result.page),
                    summary_relevancy = ?result.page.summary_relevancy,
                    summary_budget_exhausted = ?budget_exhausted,
                    "Page summarized"
                );
                total_tokens += if result.tokens == 0 {
                    page_context_tokens + page_summary_tokens(&result.page)
                } else {
                    result.tokens
                };
                total_attempts += result.attempts_used;
                if let Some(relevancy) = result.page.summary_relevancy {
                    relevancies.push(relevancy as usize);
                }
                let mut summarized_page = result.page;
                summarized_page.summary_budget_exhausted =
                    budget_exhausted.map(|reason| reason.to_string());
                summarized_pages.push(summarized_page);
                emit_progress(
                    progress.as_ref(),
                    progress_payload(
                        &stages,
                        PipelineProgressStage::Summarization,
                        Some(page_number),
                        Some(total_pages),
                        index + 1,
                        total_pages.max(1),
                        format!("Summarized page {page_number} of {total_pages}."),
                    ),
                );
            }
            output.pages = summarized_pages;
            summarization_metrics.duration_ms = summarization_started.elapsed().as_millis() as u64;
            summarization_metrics.pages_processed = output.pages.len();
            summarization_metrics.tokens = total_tokens;
            summarization_metrics.avg_relevancy = average_percent(&relevancies);
            summarization_metrics.total_attempts = Some(total_attempts);
            tracing::info!(
                target: "summarizer_pipeline",
                job_id,
                stage = "summarization",
                duration_ms = summarization_metrics.duration_ms,
                pages = summarization_metrics.pages_processed,
                tokens = summarization_metrics.tokens,
                attempts = total_attempts,
                "Summarization completed"
            );
        }

        output.metrics = Some(PipelineMetrics {
            total_duration_ms: started.elapsed().as_millis() as u64,
            // Token metrics intentionally exclude base64 image payload bytes. They cover
            // text/table context, vision model usage, and summarization model usage only.
            total_tokens: content_tokens(&output.pages)
                + vision_metrics.tokens
                + summarization_metrics.tokens,
            stages: PipelineStages {
                extraction: StageMetrics {
                    duration_ms: extraction_duration.as_millis() as u64,
                    pages_processed: output.pages.len(),
                    tokens: content_tokens(&output.pages),
                    pages_with_images: None,
                    classified_count: None,
                    extracted_count: None,
                    avg_relevancy: None,
                    total_attempts: None,
                },
                vision: vision_metrics,
                summarization: summarization_metrics,
            },
            config: PipelineMetricsConfig {
                vision_mode: Some(serde_name(config.vision_mode)),
                vision_classifier_provider: resolved_vision_classifier_provider(config),
                vision_extractor_provider: resolved_vision_extractor_provider(config),
                summarizer_provider: resolved_summarizer_provider(config),
                summarizer_mode: Some(serde_name(config.summarizer_mode)),
            },
        });

        if !config.keep_base64_images {
            for page in &mut output.pages {
                strip_page_base64_images(page);
            }
        }

        let metrics = output.metrics.as_ref();
        tracing::info!(
            target: "summarizer_pipeline",
            job_id,
            duration_ms = started.elapsed().as_millis() as u64,
            pages = output.pages.len(),
            total_tokens = metrics.map(|metrics| metrics.total_tokens).unwrap_or_default(),
            extraction_tokens = metrics
                .map(|metrics| metrics.stages.extraction.tokens)
                .unwrap_or_default(),
            vision_tokens = metrics
                .map(|metrics| metrics.stages.vision.tokens)
                .unwrap_or_default(),
            summarization_tokens = metrics
                .map(|metrics| metrics.stages.summarization.tokens)
                .unwrap_or_default(),
            pages_with_images = ?metrics.and_then(|metrics| metrics.stages.vision.pages_with_images),
            classified_count = ?metrics.and_then(|metrics| metrics.stages.vision.classified_count),
            extracted_count = ?metrics.and_then(|metrics| metrics.stages.vision.extracted_count),
            summarization_attempts = ?metrics.and_then(|metrics| metrics.stages.summarization.total_attempts),
            avg_relevancy = ?metrics.and_then(|metrics| metrics.stages.summarization.avg_relevancy),
            "Pipeline completed"
        );
        Ok(output)
    }
}

impl Default for Pipeline {
    fn default() -> Self {
        Self::new()
    }
}

fn strip_page_base64_images(page: &mut PageOutput) {
    page.image_base64 = None;
    for image in &mut page.embedded_images {
        image.base64 = None;
    }
}

fn serde_name<T: serde::Serialize>(value: T) -> String {
    serde_json::to_value(value)
        .ok()
        .and_then(|value| value.as_str().map(ToString::to_string))
        .unwrap_or_else(|| "unknown".to_string())
}

fn resolved_vision_classifier_provider(config: &PipelineConfig) -> Option<String> {
    if config.vision_mode == VisionMode::None
        || config.extract_only
        || config.vision_skip_classification
    {
        return None;
    }
    Some(serde_name(resolve_vision_classifier_mode(config)))
}

fn resolved_vision_extractor_provider(config: &PipelineConfig) -> Option<String> {
    if config.vision_mode == VisionMode::None || config.extract_only {
        return None;
    }
    Some(serde_name(resolve_vision_extractor_mode(config)))
}

fn resolved_summarizer_provider(config: &PipelineConfig) -> Option<String> {
    if !config.run_summarization || config.summarizer_mode == SummarizerMode::Skip {
        return None;
    }
    Some(serde_name(resolve_summarizer_provider(config)))
}

async fn run_vision_stage(
    pages: &mut [summarizer_types::PageOutput],
    config: &PipelineConfig,
    provider_config: &PipelineProviderConfig,
    stages: &[PipelineProgressStage],
    progress: &(dyn Fn(PipelineProgress) + Send + Sync),
    job_id: &str,
) -> Result<StageMetrics, PipelineError> {
    if config.vision_mode == VisionMode::None || config.extract_only {
        return Ok(StageMetrics::empty());
    }

    emit_progress(
        progress,
        progress_payload(
            stages,
            PipelineProgressStage::Vision,
            None,
            Some(pages.len()),
            0,
            pages.len().max(1),
            "Starting vision processing.",
        ),
    );

    let pages_with_images = pages
        .iter()
        .filter(|page| {
            page.image_base64
                .as_deref()
                .is_some_and(|image| !image.is_empty())
        })
        .count();
    if pages_with_images == 0 {
        tracing::info!(
            target: "summarizer_pipeline",
            job_id,
            stage = "vision",
            pages = pages.len(),
            "Vision skipped because no page images were available"
        );
        emit_progress(
            progress,
            progress_payload(
                stages,
                PipelineProgressStage::Vision,
                None,
                Some(pages.len()),
                1,
                1,
                "Vision skipped because no page images were available.",
            ),
        );
        return Ok(StageMetrics {
            pages_with_images: Some(0),
            ..StageMetrics::empty()
        });
    }

    let classifier_mode = resolve_vision_classifier_mode(config);
    let extractor_mode = resolve_vision_extractor_mode(config);
    let classifier_provider_name = if config.vision_skip_classification {
        None
    } else {
        Some(serde_name(classifier_mode))
    };
    let extractor_provider_name = serde_name(extractor_mode);
    let extractor_provider = build_vision_provider(extractor_mode, provider_config)?;
    let classifier_provider = if config.vision_skip_classification {
        None
    } else {
        Some(build_vision_provider(classifier_mode, provider_config)?)
    };
    let started = Instant::now();
    tracing::info!(
        target: "summarizer_pipeline",
        job_id,
        stage = "vision",
        pages = pages.len(),
        pages_with_images,
        classifier_provider = ?classifier_provider_name,
        extractor_provider = %extractor_provider_name,
        skip_classification = config.vision_skip_classification,
        "Vision started"
    );
    let mut classified_count = 0;
    let mut extracted_count = 0;

    let total_pages = pages.len();
    let mut processed_pages = 0;
    for (index, page) in pages.iter_mut().enumerate() {
        let page_number = page.page_number.unwrap_or(index + 1);
        let Some(image_base64) = page.image_base64.clone().filter(|image| !image.is_empty()) else {
            tracing::info!(
                target: "summarizer_pipeline",
                job_id,
                stage = "vision",
                page_number,
                total_pages,
                chunk_id = %page.chunk_id,
                "Vision page skipped because no page image was available"
            );
            continue;
        };
        let page_started = Instant::now();
        tracing::info!(
            target: "summarizer_pipeline",
            job_id,
            stage = "vision",
            page_number,
            total_pages,
            chunk_id = %page.chunk_id,
            image_base64_chars = image_base64.len(),
            classifier_provider = ?classifier_provider_name,
            extractor_provider = %extractor_provider_name,
            skip_classification = config.vision_skip_classification,
            "Vision page started"
        );
        emit_progress(
            progress,
            progress_payload(
                stages,
                PipelineProgressStage::Vision,
                Some(page_number),
                Some(total_pages),
                processed_pages,
                pages_with_images,
                format!("Analyzing page {page_number} of {total_pages}."),
            ),
        );

        let vision_page = VisionPage {
            page_number: page.page_number.unwrap_or(0),
            chunk_id: page.chunk_id.clone(),
            image_base64,
        };

        let should_extract = if config.vision_skip_classification {
            true
        } else {
            let classification_started = Instant::now();
            match classifier_provider
                .as_ref()
                .expect("classifier provider is available when classification is enabled")
                .classify(&vision_page)
                .await
            {
                Ok(classification) => {
                    tracing::info!(
                        target: "summarizer_pipeline",
                        job_id,
                        stage = "vision",
                        page_number,
                        total_pages,
                        chunk_id = %page.chunk_id,
                        classifier_provider = ?classifier_provider_name,
                        duration_ms = classification_started.elapsed().as_millis() as u64,
                        has_graphics = classification.has_graphics,
                        "Page classified"
                    );
                    classified_count += 1;
                    page.image_classifier = Some(classification.has_graphics);
                    classification.has_graphics
                }
                // Degrade-and-continue: a classifier failure must not abort the
                // whole job. Fall back to extracting the page (matching the
                // skip-classification default) rather than dropping it.
                Err(err) => {
                    tracing::warn!(
                        target: "summarizer_pipeline",
                        job_id,
                        stage = "vision",
                        page_number,
                        total_pages,
                        chunk_id = %page.chunk_id,
                        classifier_provider = ?classifier_provider_name,
                        duration_ms = classification_started.elapsed().as_millis() as u64,
                        error = %err,
                        "Page classification failed after retries; extracting without classification"
                    );
                    page.image_classifier = None;
                    true
                }
            }
        };

        if should_extract {
            let extraction_started = Instant::now();
            match extractor_provider.extract(&vision_page).await {
                Ok(extraction) => {
                    page.image_text = extraction.image_text;
                    tracing::info!(
                        target: "summarizer_pipeline",
                        job_id,
                        stage = "vision",
                        page_number,
                        total_pages,
                        chunk_id = %page.chunk_id,
                        extractor_provider = %extractor_provider_name,
                        duration_ms = extraction_started.elapsed().as_millis() as u64,
                        image_text_chars = optional_text_chars(page.image_text.as_deref()),
                        "Vision page extraction completed"
                    );
                    tracing::info!(
                        target: "summarizer_pipeline",
                        job_id,
                        stage = "vision",
                        page_number,
                        total_pages,
                        chunk_id = %page.chunk_id,
                        image_text_chars = optional_text_chars(page.image_text.as_deref()),
                        "Page vision extraction completed"
                    );
                    extracted_count += 1;
                }
                // Degrade-and-continue: keep the page with no vision text rather
                // than failing the entire job on one stubborn page.
                Err(err) => {
                    page.image_text = None;
                    tracing::warn!(
                        target: "summarizer_pipeline",
                        job_id,
                        stage = "vision",
                        page_number,
                        total_pages,
                        chunk_id = %page.chunk_id,
                        extractor_provider = %extractor_provider_name,
                        duration_ms = extraction_started.elapsed().as_millis() as u64,
                        error = %err,
                        "Vision page extraction failed after retries; continuing with no image text"
                    );
                }
            }
        } else {
            tracing::info!(
                target: "summarizer_pipeline",
                job_id,
                stage = "vision",
                page_number,
                total_pages,
                chunk_id = %page.chunk_id,
                reason = "classification_false",
                "Vision page extraction skipped"
            );
        }
        if let Some(provider) = classifier_provider.as_ref() {
            provider.release_page(&vision_page)?;
        }
        extractor_provider.release_page(&vision_page)?;
        processed_pages += 1;
        tracing::info!(
            target: "summarizer_pipeline",
            job_id,
            stage = "vision",
            page_number,
            total_pages,
            chunk_id = %page.chunk_id,
            duration_ms = page_started.elapsed().as_millis() as u64,
            extracted = should_extract,
            image_classifier = ?page.image_classifier,
            image_text_chars = optional_text_chars(page.image_text.as_deref()),
            "Vision page completed"
        );
        emit_progress(
            progress,
            progress_payload(
                stages,
                PipelineProgressStage::Vision,
                Some(page_number),
                Some(total_pages),
                processed_pages,
                pages_with_images,
                format!("Finished vision for page {page_number} of {total_pages}."),
            ),
        );
    }

    let metrics = StageMetrics {
        duration_ms: started.elapsed().as_millis() as u64,
        pages_processed: pages.len(),
        tokens: vision_content_tokens(pages),
        pages_with_images: Some(pages_with_images),
        classified_count: Some(classified_count),
        extracted_count: Some(extracted_count),
        avg_relevancy: None,
        total_attempts: None,
    };
    tracing::info!(
        target: "summarizer_pipeline",
        job_id,
        stage = "vision",
        duration_ms = metrics.duration_ms,
        pages_with_images,
        classified_count,
        extracted_count,
        tokens = metrics.tokens,
        "Vision completed"
    );
    Ok(metrics)
}

fn resolve_vision_classifier_mode(config: &PipelineConfig) -> VisionMode {
    resolve_cli_vision_mode(
        config.vision_classifier_mode.unwrap_or(config.vision_mode),
        config.vision_cli_provider,
    )
}

fn resolve_vision_extractor_mode(config: &PipelineConfig) -> VisionMode {
    resolve_cli_vision_mode(
        config.vision_extractor_mode.unwrap_or(config.vision_mode),
        config.vision_cli_provider,
    )
}

fn resolve_cli_vision_mode(mode: VisionMode, cli_provider: Option<CliProvider>) -> VisionMode {
    match (mode, cli_provider) {
        (
            VisionMode::Codex | VisionMode::Claude | VisionMode::Grok | VisionMode::Copilot,
            Some(CliProvider::Codex),
        ) => VisionMode::Codex,
        (
            VisionMode::Codex | VisionMode::Claude | VisionMode::Grok | VisionMode::Copilot,
            Some(CliProvider::Claude),
        ) => VisionMode::Claude,
        (
            VisionMode::Codex | VisionMode::Claude | VisionMode::Grok | VisionMode::Copilot,
            Some(CliProvider::Grok),
        ) => VisionMode::Grok,
        (
            VisionMode::Codex | VisionMode::Claude | VisionMode::Grok | VisionMode::Copilot,
            Some(CliProvider::Copilot),
        ) => VisionMode::Copilot,
        _ => mode,
    }
}

fn build_vision_provider(
    mode: VisionMode,
    provider_config: &PipelineProviderConfig,
) -> Result<Box<dyn VisionProvider>, PipelineError> {
    match mode {
        VisionMode::None => unreachable!("none vision mode is handled before provider creation"),
        VisionMode::LlamaCpp => Ok(Box::new(
            OpenAiCompatibleVisionProvider::new(
                provider_config.llama_cpp.vision_base_url.clone(),
                provider_config.llama_cpp.api_key.clone(),
                provider_config.llama_cpp.vision_model.clone(),
            )
            .with_llama_cpp_options(),
        )),
        VisionMode::Openai => Ok(Box::new(OpenAiCompatibleVisionProvider::new(
            provider_config.openai.base_url.clone(),
            provider_config.openai.api_key.clone(),
            provider_config.openai.vision_model.clone(),
        ))),
        VisionMode::Ollama => Ok(Box::new(OpenAiCompatibleVisionProvider::new(
            provider_config.ollama.openai_base_url.clone(),
            provider_config.ollama.api_key.clone(),
            provider_config.ollama.vision_model.clone(),
        ))),
        VisionMode::Gemini => Ok(Box::new(GeminiVisionProvider::new(
            provider_config.gemini.base_url.clone(),
            provider_config.gemini.api_key.clone(),
            provider_config.gemini.vision_model.clone(),
        ))),
        VisionMode::Codex => Ok(Box::new(
            CliVisionProvider::codex(provider_config.codex.executable.clone())
                .with_args(provider_config.codex.args.clone())
                .with_timeout_seconds(provider_config.codex.timeout_seconds)
                .with_retries(provider_config.codex.retries),
        )),
        VisionMode::Claude => Ok(Box::new(
            CliVisionProvider::new(provider_config.claude.executable.clone())
                .with_args(provider_config.claude.args.clone())
                .with_timeout_seconds(provider_config.claude.timeout_seconds)
                .with_retries(provider_config.claude.retries),
        )),
        VisionMode::Grok => Ok(Box::new(
            CliVisionProvider::grok(provider_config.grok.executable.clone())
                .with_args(provider_config.grok.args.clone())
                .with_timeout_seconds(provider_config.grok.timeout_seconds)
                .with_retries(provider_config.grok.retries),
        )),
        VisionMode::Copilot => Ok(Box::new(
            CliVisionProvider::copilot(provider_config.copilot.executable.clone())
                .with_args(provider_config.copilot.args.clone())
                .with_timeout_seconds(provider_config.copilot.timeout_seconds)
                .with_retries(provider_config.copilot.retries),
        )),
        VisionMode::Deepseek => Err(PipelineError::Vision(
            "deepseek vision mode is out of scope for the Rust backend".to_string(),
        )),
    }
}

fn env_or(key: &str, default: impl Into<String>) -> String {
    std::env::var(key).unwrap_or_else(|_| default.into())
}

fn optional_env(key: &str) -> Option<String> {
    std::env::var(key)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

fn resolve_summarizer_provider(config: &PipelineConfig) -> SummarizerProvider {
    match (config.summarizer_provider, config.summarizer_cli_provider) {
        (
            SummarizerProvider::Codex
            | SummarizerProvider::Claude
            | SummarizerProvider::Grok
            | SummarizerProvider::Copilot,
            Some(CliProvider::Codex),
        ) => SummarizerProvider::Codex,
        (
            SummarizerProvider::Codex
            | SummarizerProvider::Claude
            | SummarizerProvider::Grok
            | SummarizerProvider::Copilot,
            Some(CliProvider::Claude),
        ) => SummarizerProvider::Claude,
        (
            SummarizerProvider::Codex
            | SummarizerProvider::Claude
            | SummarizerProvider::Grok
            | SummarizerProvider::Copilot,
            Some(CliProvider::Grok),
        ) => SummarizerProvider::Grok,
        (
            SummarizerProvider::Codex
            | SummarizerProvider::Claude
            | SummarizerProvider::Grok
            | SummarizerProvider::Copilot,
            Some(CliProvider::Copilot),
        ) => SummarizerProvider::Copilot,
        _ => config.summarizer_provider,
    }
}

fn build_summarizer(
    config: &PipelineConfig,
    provider_config: &PipelineProviderConfig,
) -> Result<Box<dyn Summarizer>, PipelineError> {
    let provider = resolve_summarizer_provider(config);
    let max_tokens_per_page =
        env_usize("SUMMARIZER_MAX_TOKENS_PER_PAGE", config.max_tokens_per_page);
    let max_seconds_per_page = env_u64(
        "SUMMARIZER_MAX_SECONDS_PER_PAGE",
        config.max_seconds_per_page,
    );
    match provider {
        SummarizerProvider::LlamaCpp => Ok(Box::new(
            OpenAiCompatibleSummarizer::new(
                provider_config.llama_cpp.base_url.clone(),
                provider_config.llama_cpp.api_key.clone(),
                provider_config.llama_cpp.model.clone(),
            )
            .with_model_tiers(
                provider_config.llama_cpp.model_2.clone(),
                provider_config.llama_cpp.model_3.clone(),
            )
            .with_budget(max_tokens_per_page, max_seconds_per_page)
            .with_llama_cpp_options(),
        )),
        SummarizerProvider::Openai => Ok(Box::new(
            OpenAiCompatibleSummarizer::new(
                provider_config.openai.base_url.clone(),
                provider_config.openai.api_key.clone(),
                provider_config.openai.model.clone(),
            )
            .with_model_tiers(
                provider_config.openai.model_2.clone(),
                provider_config.openai.model_3.clone(),
            )
            .with_budget(max_tokens_per_page, max_seconds_per_page),
        )),
        SummarizerProvider::Ollama => Ok(Box::new(
            OpenAiCompatibleSummarizer::new(
                provider_config.ollama.openai_base_url.clone(),
                provider_config.ollama.api_key.clone(),
                provider_config.ollama.model.clone(),
            )
            .with_model_tiers(
                provider_config.ollama.model_2.clone(),
                provider_config.ollama.model_3.clone(),
            )
            .with_budget(max_tokens_per_page, max_seconds_per_page),
        )),
        SummarizerProvider::Codex => Ok(Box::new(
            CliSummarizer::codex(provider_config.codex.executable.clone())
                .with_args(provider_config.codex.args.clone())
                .with_timeout_seconds(provider_config.codex.timeout_seconds)
                .with_retries(provider_config.codex.retries),
        )),
        SummarizerProvider::Claude => Ok(Box::new(
            CliSummarizer::new(provider_config.claude.executable.clone())
                .with_args(provider_config.claude.args.clone())
                .with_timeout_seconds(provider_config.claude.timeout_seconds)
                .with_retries(provider_config.claude.retries),
        )),
        SummarizerProvider::Grok => Ok(Box::new(
            CliSummarizer::grok(provider_config.grok.executable.clone())
                .with_args(provider_config.grok.args.clone())
                .with_timeout_seconds(provider_config.grok.timeout_seconds)
                .with_retries(provider_config.grok.retries),
        )),
        SummarizerProvider::Copilot => Ok(Box::new(
            CliSummarizer::copilot(provider_config.copilot.executable.clone())
                .with_args(provider_config.copilot.args.clone())
                .with_timeout_seconds(provider_config.copilot.timeout_seconds)
                .with_retries(provider_config.copilot.retries),
        )),
    }
}

fn summary_budget_reason(reason: SummarizationBudgetExhaustReason) -> &'static str {
    match reason {
        SummarizationBudgetExhaustReason::TokensExceeded => "tokens",
        SummarizationBudgetExhaustReason::TimeExceeded => "time",
    }
}

fn progress_stages(config: &PipelineConfig) -> Vec<PipelineProgressStage> {
    let mut stages = vec![PipelineProgressStage::Extraction];
    if config.vision_mode != VisionMode::None && !config.extract_only {
        stages.push(PipelineProgressStage::Vision);
    }
    if config.run_summarization
        && config.summarizer_mode != SummarizerMode::Skip
        && !config.extract_only
    {
        stages.push(PipelineProgressStage::Summarization);
    }
    stages
}

fn progress_payload(
    stages: &[PipelineProgressStage],
    stage: PipelineProgressStage,
    page_number: Option<usize>,
    total_pages: Option<usize>,
    completed_in_stage: usize,
    total_in_stage: usize,
    message: impl Into<String>,
) -> PipelineProgress {
    let stage_index = stages
        .iter()
        .position(|candidate| *candidate == stage)
        .unwrap_or(0);
    let total_stages = stages.len().max(1);
    PipelineProgress {
        stage,
        stage_index,
        total_stages,
        page_number,
        total_pages,
        progress: progress_percent(
            stage_index,
            total_stages,
            completed_in_stage,
            total_in_stage,
        ),
        message: message.into(),
    }
}

fn extraction_progress_callback(
    stages: &[PipelineProgressStage],
    progress: Arc<dyn Fn(PipelineProgress) + Send + Sync>,
) -> Arc<dyn Fn(ExtractionProgress) + Send + Sync> {
    let stages = stages.to_vec();
    Arc::new(move |event| {
        emit_progress(
            progress.as_ref(),
            progress_payload(
                &stages,
                PipelineProgressStage::Extraction,
                Some(event.page_number),
                Some(event.total_pages),
                event.completed_pages,
                event.total_pages,
                event.message,
            ),
        );
    })
}

fn progress_percent(
    stage_index: usize,
    total_stages: usize,
    completed_in_stage: usize,
    total_in_stage: usize,
) -> u8 {
    let total_stages = total_stages.max(1);
    let stage_fraction = completed_in_stage as f64 / total_in_stage.max(1) as f64;
    let value = ((stage_index as f64 + stage_fraction.clamp(0.0, 1.0)) / total_stages as f64
        * 100.0)
        .round();
    value.clamp(1.0, 99.0) as u8
}

fn emit_progress(progress: &(dyn Fn(PipelineProgress) + Send + Sync), payload: PipelineProgress) {
    tracing::debug!(
        target: "summarizer_pipeline",
        stage = payload.stage.as_str(),
        stage_index = payload.stage_index,
        total_stages = payload.total_stages,
        page_number = ?payload.page_number,
        total_pages = ?payload.total_pages,
        progress = payload.progress,
        message = %payload.message,
        "Pipeline progress"
    );
    progress(payload);
}

fn content_tokens(pages: &[summarizer_types::PageOutput]) -> usize {
    pages.iter().map(page_content_tokens).sum()
}

fn optional_text_chars(value: Option<&str>) -> usize {
    value.map(|value| value.chars().count()).unwrap_or(0)
}

fn page_table_rows(page: &summarizer_types::PageOutput) -> usize {
    page.tables.iter().map(Vec::len).sum()
}

fn page_has_image(page: &summarizer_types::PageOutput) -> bool {
    page.image_base64
        .as_deref()
        .is_some_and(|image| !image.is_empty())
}

fn page_image_base64_chars(page: &summarizer_types::PageOutput) -> usize {
    optional_text_chars(page.image_base64.as_deref())
}

fn page_vision_text_chars(page: &summarizer_types::PageOutput) -> usize {
    [
        page.image_text.as_deref(),
        page.image_text_1.as_deref(),
        page.image_text_2.as_deref(),
        page.image_text_3.as_deref(),
    ]
    .into_iter()
    .flatten()
    .map(|value| value.chars().count())
    .sum()
}

fn summary_notes_count(page: &summarizer_types::PageOutput) -> usize {
    page.summary_notes.as_ref().map(Vec::len).unwrap_or(0)
}

fn summary_topics_count(page: &summarizer_types::PageOutput) -> usize {
    page.summary_topics.as_ref().map(Vec::len).unwrap_or(0)
}

fn page_content_tokens(page: &summarizer_types::PageOutput) -> usize {
    estimate_tokens(&page.text)
        + page
            .tables
            .iter()
            .flatten()
            .flatten()
            .map(|cell| estimate_tokens(cell))
            .sum::<usize>()
}

fn vision_content_tokens(pages: &[summarizer_types::PageOutput]) -> usize {
    pages.iter().map(page_vision_tokens).sum()
}

fn page_vision_tokens(page: &summarizer_types::PageOutput) -> usize {
    let classification_tokens = usize::from(page.image_classifier.is_some());
    classification_tokens
        + [
            page.image_text.as_deref(),
            page.image_text_1.as_deref(),
            page.image_text_2.as_deref(),
            page.image_text_3.as_deref(),
        ]
        .into_iter()
        .flatten()
        .map(estimate_tokens)
        .sum::<usize>()
}

fn page_summary_tokens(page: &summarizer_types::PageOutput) -> usize {
    page.summary_notes
        .as_deref()
        .unwrap_or(&[])
        .iter()
        .map(|note| estimate_tokens(note))
        .sum::<usize>()
        + page
            .summary_topics
            .as_deref()
            .unwrap_or(&[])
            .iter()
            .map(|topic| estimate_tokens(topic))
            .sum::<usize>()
}

fn estimate_tokens(value: &str) -> usize {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return 0;
    }
    let word_estimate = trimmed.split_whitespace().count();
    let char_estimate = trimmed.chars().count().div_ceil(4);
    word_estimate.max(char_estimate).max(1)
}

fn average_percent(values: &[usize]) -> Option<u8> {
    if values.is_empty() {
        None
    } else {
        Some((values.iter().sum::<usize>() / values.len()).min(100) as u8)
    }
}

fn env_args(key: &str) -> Vec<String> {
    std::env::var(key)
        .map(|value| {
            value
                .split_whitespace()
                .filter(|arg| !arg.is_empty())
                .map(ToString::to_string)
                .collect()
        })
        .unwrap_or_default()
}

fn env_usize(key: &str, default: usize) -> usize {
    std::env::var(key)
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(default)
}

fn env_u64(key: &str, default: u64) -> u64 {
    std::env::var(key)
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(default)
}

fn env_u32(key: &str, default: u32) -> u32 {
    std::env::var(key)
        .ok()
        .and_then(|value| value.parse().ok())
        .map(|value: u32| value.max(1))
        .unwrap_or(default)
}
