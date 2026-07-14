use async_trait::async_trait;
use reqwest::{Client, Response, StatusCode};
use serde::{de::DeserializeOwned, Deserialize};
use serde_json::json;
use std::process::Stdio;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use summarizer_cli_util::{
    cli_command_context, configure_isolated_grok_command, create_isolated_grok_home,
    parse_codex_jsonl_output, parse_grok_json, resolve_cli_executable, run_cli_command_with_retry,
    RetryPolicy,
};
use summarizer_types::{PageOutput, PipelineError, SummarizerMode};
use tokio::process::Command;

const NOTES_PROMPT: &str = include_str!("../../../prompts/summarizer-notes-prompt.txt");
const NOTES_FALLBACK_PROMPT: &str =
    include_str!("../../../prompts/summarizer-notes-prompt-fallback.txt");
const TOPICS_PROMPT: &str = include_str!("../../../prompts/summarizer-topics-prompt.txt");
const SYNTHESIS_PROMPT: &str = include_str!("../../../prompts/summarizer-synthesis.txt");
const INSIGHT_PROMPT: &str = include_str!("../../../prompts/summarizer-insight-prompt.txt");
const MAX_PROVIDER_ERROR_BODY_BYTES: usize = 2048;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CliProviderKind {
    Generic,
    Codex,
    Grok,
    Copilot,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QualityPolicy {
    pub high_threshold: u8,
    pub low_threshold: u8,
    pub high_threshold_attempts: usize,
    pub tier_1_max_attempt: usize,
    pub tier_2_max_attempt: usize,
}

impl Default for QualityPolicy {
    fn default() -> Self {
        Self {
            high_threshold: 90,
            low_threshold: 85,
            high_threshold_attempts: 5,
            tier_1_max_attempt: 10,
            tier_2_max_attempt: 20,
        }
    }
}

impl QualityPolicy {
    pub fn threshold_for_attempt(&self, attempt: usize) -> u8 {
        if attempt <= self.high_threshold_attempts {
            self.high_threshold
        } else {
            self.low_threshold
        }
    }

    pub fn tier_for_attempt(&self, attempt: usize) -> u8 {
        if attempt <= self.tier_1_max_attempt {
            1
        } else if attempt <= self.tier_2_max_attempt {
            2
        } else {
            3
        }
    }

    pub fn prompt_for_attempt(&self, attempt: usize) -> SummaryPrompt {
        if attempt <= self.high_threshold_attempts {
            SummaryPrompt::Primary
        } else {
            SummaryPrompt::Fallback
        }
    }

    pub fn validates(&self, relevancy: u8, attempt: usize) -> bool {
        relevancy >= self.threshold_for_attempt(attempt)
    }

    fn band_for_attempt(&self, attempt: usize) -> AttemptBand {
        AttemptBand {
            prompt: self.prompt_for_attempt(attempt),
            tier: self.tier_for_attempt(attempt),
            threshold: self.threshold_for_attempt(attempt),
        }
    }

    fn next_band_start_after(&self, attempt: usize, max_attempts: usize) -> Option<usize> {
        let current = self.band_for_attempt(attempt);
        ((attempt + 1)..=max_attempts)
            .find(|next_attempt| self.band_for_attempt(*next_attempt) != current)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SummaryPrompt {
    Primary,
    Fallback,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct AttemptBand {
    prompt: SummaryPrompt,
    tier: u8,
    threshold: u8,
}

#[derive(Debug, Clone, PartialEq)]
pub struct SummarizationResult {
    pub page: PageOutput,
    pub attempts_used: usize,
    pub tokens: usize,
    pub budget_exhausted: Option<SummarizationBudgetExhaustReason>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SummarizationBudgetExhaustReason {
    TokensExceeded,
    TimeExceeded,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SummarizationOptions {
    pub mode: SummarizerMode,
    pub detailed_extraction: bool,
    pub insight_mode: bool,
}

impl SummarizationOptions {
    pub fn new(mode: SummarizerMode) -> Self {
        Self {
            mode,
            detailed_extraction: false,
            insight_mode: false,
        }
    }
}

#[async_trait]
pub trait Summarizer: Send + Sync {
    async fn summarize_page(
        &self,
        page: &PageOutput,
        options: SummarizationOptions,
    ) -> Result<SummarizationResult, PipelineError>;

    fn reported_model(&self) -> Option<String> {
        None
    }
}

#[derive(Debug, Clone)]
pub struct PassthroughSummarizer;

#[async_trait]
impl Summarizer for PassthroughSummarizer {
    async fn summarize_page(
        &self,
        page: &PageOutput,
        options: SummarizationOptions,
    ) -> Result<SummarizationResult, PipelineError> {
        let mode = options.mode;
        let mut page = page.clone();
        if mode == SummarizerMode::Skip {
            page.summary_notes = None;
            page.summary_topics = None;
            page.summary_relevancy = Some(0);
            page.summary_quality_validated = None;
            return Ok(SummarizationResult {
                page,
                attempts_used: 0,
                tokens: 0,
                budget_exhausted: None,
            });
        }

        if page.summary_notes.is_none() && !page.text.trim().is_empty() {
            if mode != SummarizerMode::TopicsOnly {
                page.summary_notes = Some(vec![page
                    .text
                    .trim()
                    .lines()
                    .next()
                    .unwrap_or_default()
                    .to_string()]);
            }
            page.summary_topics = Some(Vec::new());
            page.summary_relevancy = Some(100);
            page.summary_quality_validated = Some(false);
        }
        Ok(SummarizationResult {
            page,
            attempts_used: 1,
            tokens: 0,
            budget_exhausted: None,
        })
    }
}

#[derive(Debug, Clone)]
pub struct OpenAiCompatibleSummarizer {
    client: Client,
    base_url: String,
    api_key: Option<String>,
    model_tier_1: String,
    model_tier_2: String,
    model_tier_3: String,
    max_attempts: usize,
    max_tokens_per_page: usize,
    max_seconds_per_page: u64,
    quality_policy: QualityPolicy,
    disable_thinking: bool,
}

impl OpenAiCompatibleSummarizer {
    pub fn new(
        base_url: impl Into<String>,
        api_key: Option<String>,
        model_tier_1: impl Into<String>,
    ) -> Self {
        let model_tier_1 = model_tier_1.into();
        Self {
            client: summarizer_http_client(Duration::from_secs(
                summarizer_types::default_max_seconds_per_page(),
            )),
            base_url: base_url.into().trim_end_matches('/').to_string(),
            api_key,
            model_tier_2: model_tier_1.clone(),
            model_tier_3: model_tier_1.clone(),
            model_tier_1,
            max_attempts: 30,
            max_tokens_per_page: summarizer_types::default_max_tokens_per_page(),
            max_seconds_per_page: summarizer_types::default_max_seconds_per_page(),
            quality_policy: QualityPolicy::default(),
            disable_thinking: false,
        }
    }

    pub fn with_model_tiers(
        mut self,
        model_tier_2: impl Into<String>,
        model_tier_3: impl Into<String>,
    ) -> Self {
        self.model_tier_2 = model_tier_2.into();
        self.model_tier_3 = model_tier_3.into();
        self
    }

    pub fn with_max_attempts(mut self, max_attempts: usize) -> Self {
        self.max_attempts = max_attempts;
        self
    }

    pub fn with_budget(mut self, max_tokens_per_page: usize, max_seconds_per_page: u64) -> Self {
        self.max_tokens_per_page = max_tokens_per_page;
        self.max_seconds_per_page = max_seconds_per_page;
        self
    }

    pub fn with_llama_cpp_options(mut self) -> Self {
        self.disable_thinking = true;
        self
    }

    pub fn with_http_timeout(mut self, timeout: Duration) -> Self {
        self.client = summarizer_http_client(timeout);
        self
    }

    fn model_for_tier(&self, tier: u8) -> &str {
        match tier {
            1 => &self.model_tier_1,
            2 => &self.model_tier_2,
            _ => &self.model_tier_3,
        }
    }

    async fn chat(&self, prompt: &str, tier: u8) -> Result<LlmResponse, PipelineError> {
        let url = format!("{}/chat/completions", self.base_url);
        let mut body = json!({
            "model": self.model_for_tier(tier),
            "messages": [{"role": "user", "content": prompt}],
            "temperature": 0
        });
        if self.disable_thinking {
            body["chat_template_kwargs"] = json!({"enable_thinking": false});
        }
        let mut request = self.client.post(url).json(&body);

        if let Some(api_key) = &self.api_key {
            request = request.bearer_auth(api_key);
        }

        let response = request.send().await.map_err(summarizer_http_error)?;
        let value: ChatCompletionResponse =
            summarizer_json_response(response, "summarizer provider").await?;

        let content = value
            .choices
            .first()
            .and_then(|choice| choice.message.content.as_deref())
            .map(clean_response_content)
            .ok_or_else(|| {
                PipelineError::Summarization("missing chat completion content".to_string())
            })?;

        Ok(LlmResponse {
            content,
            tokens: value.usage.map(|usage| usage.total_tokens).unwrap_or(0),
        })
    }

    async fn summarize_full(
        &self,
        page: &PageOutput,
    ) -> Result<SummarizationResult, PipelineError> {
        let context = page_context(page);
        if context.trim().is_empty() {
            let mut page = page.clone();
            page.summary_notes = None;
            page.summary_topics = None;
            page.summary_relevancy = Some(0);
            page.summary_quality_validated = None;
            return Ok(SummarizationResult {
                page,
                attempts_used: 0,
                tokens: 0,
                budget_exhausted: None,
            });
        }

        let started = Instant::now();
        let mut attempts_used = 0;
        let mut tokens = 0;
        let mut best_notes = Vec::new();
        let mut best_relevancy = 0;
        let mut previous_score: Option<(AttemptBand, u8)> = None;

        let mut attempt = 1;
        while attempt <= self.max_attempts {
            attempts_used = attempt;
            let band = self.quality_policy.band_for_attempt(attempt);
            let prompt =
                render_notes_prompt(self.quality_policy.prompt_for_attempt(attempt), &context);
            let notes_response = self.chat(&prompt, band.tier).await?;
            tokens += notes_response.tokens;
            let notes = clean_bulleted_output(&notes_response.content);
            if notes.is_empty() {
                if let Some(reason) = self.budget_exhausted(tokens, started) {
                    return Ok(self.exhausted_result(
                        page,
                        best_notes,
                        best_relevancy,
                        attempts_used,
                        tokens,
                        reason,
                    ));
                }
                attempt += 1;
                continue;
            }
            if best_notes.is_empty() {
                best_notes = notes.clone();
            }
            if let Some(reason) = self.budget_exhausted(tokens, started) {
                return Ok(self.exhausted_result(
                    page,
                    best_notes,
                    best_relevancy,
                    attempts_used,
                    tokens,
                    reason,
                ));
            }

            let quality_prompt = render_quality_prompt(&context, &notes_response.content);
            let quality_response = self.chat(&quality_prompt, 3).await?;
            tokens += quality_response.tokens;
            let relevancy = match parse_percent(&quality_response.content) {
                Some(relevancy) => relevancy,
                None => {
                    tracing::warn!(
                        page_number = page.page_number,
                        attempt,
                        response = %quality_response.content,
                        "failed to parse relevancy score"
                    );
                    0
                }
            };
            if relevancy > best_relevancy {
                best_relevancy = relevancy;
                best_notes = notes.clone();
            }
            if let Some(reason) = self.budget_exhausted(tokens, started) {
                return Ok(self.exhausted_result(
                    page,
                    best_notes,
                    best_relevancy,
                    attempts_used,
                    tokens,
                    reason,
                ));
            }

            if self.quality_policy.validates(relevancy, attempt) {
                let topics_result = self.generate_topics(&notes.join("\n")).await?;
                tokens += topics_result.tokens;
                let mut page = page.clone();
                page.summary_notes = Some(notes);
                page.summary_topics = topics_result.topics;
                page.summary_relevancy = Some(relevancy);
                page.summary_quality_validated = Some(true);
                return Ok(SummarizationResult {
                    page,
                    attempts_used,
                    tokens,
                    budget_exhausted: self.budget_exhausted(tokens, started),
                });
            }

            if previous_score == Some((band, relevancy)) {
                if let Some(next_attempt) = self
                    .quality_policy
                    .next_band_start_after(attempt, self.max_attempts)
                {
                    tracing::debug!(
                        page_number = page.page_number,
                        attempt,
                        next_attempt,
                        relevancy,
                        threshold = band.threshold,
                        "skipping repeated summarization quality band"
                    );
                    previous_score = None;
                    attempt = next_attempt;
                    continue;
                }
                break;
            }

            previous_score = Some((band, relevancy));
            attempt += 1;
        }

        let topics_result = if best_notes.is_empty() {
            TopicsResult {
                topics: None,
                tokens: 0,
            }
        } else {
            self.generate_topics(&best_notes.join("\n")).await?
        };
        tokens += topics_result.tokens;

        let mut page = page.clone();
        page.summary_notes = if best_notes.is_empty() {
            None
        } else {
            Some(best_notes)
        };
        page.summary_topics = topics_result.topics;
        page.summary_relevancy = Some(best_relevancy);
        page.summary_quality_validated = Some(false);
        Ok(SummarizationResult {
            page,
            attempts_used,
            tokens,
            budget_exhausted: None,
        })
    }

    fn budget_exhausted(
        &self,
        tokens: usize,
        started: Instant,
    ) -> Option<SummarizationBudgetExhaustReason> {
        if tokens > self.max_tokens_per_page {
            return Some(SummarizationBudgetExhaustReason::TokensExceeded);
        }
        if started.elapsed() > Duration::from_secs(self.max_seconds_per_page) {
            return Some(SummarizationBudgetExhaustReason::TimeExceeded);
        }
        None
    }

    fn exhausted_result(
        &self,
        source: &PageOutput,
        best_notes: Vec<String>,
        best_relevancy: u8,
        attempts_used: usize,
        tokens: usize,
        reason: SummarizationBudgetExhaustReason,
    ) -> SummarizationResult {
        let mut page = source.clone();
        page.summary_notes = if best_notes.is_empty() {
            None
        } else {
            Some(best_notes)
        };
        page.summary_topics = None;
        page.summary_relevancy = Some(best_relevancy);
        page.summary_quality_validated = Some(false);
        SummarizationResult {
            page,
            attempts_used,
            tokens,
            budget_exhausted: Some(reason),
        }
    }

    async fn summarize_topics_only(
        &self,
        page: &PageOutput,
    ) -> Result<SummarizationResult, PipelineError> {
        let context = page_context(page);
        let topics_result = self
            .generate_topics(&context.chars().take(2000).collect::<String>())
            .await?;
        let mut page = page.clone();
        page.summary_notes = None;
        page.summary_topics = topics_result.topics;
        page.summary_relevancy = Some(0);
        page.summary_quality_validated = None;
        Ok(SummarizationResult {
            page,
            attempts_used: 1,
            tokens: topics_result.tokens,
            budget_exhausted: None,
        })
    }

    async fn summarize_detailed(
        &self,
        page: &PageOutput,
    ) -> Result<SummarizationResult, PipelineError> {
        let first = self.summarize_full(page).await?;
        let second = self.summarize_full(page).await?;
        let third = self.summarize_full(page).await?;

        let notes_1 = first.page.summary_notes.clone().unwrap_or_default();
        let notes_2 = second.page.summary_notes.clone().unwrap_or_default();
        let notes_3 = third.page.summary_notes.clone().unwrap_or_default();
        let extraction_text = format!(
            "Extraction 1:\n{}\n\nExtraction 2:\n{}\n\nExtraction 3:\n{}",
            notes_1.join("\n"),
            notes_2.join("\n"),
            notes_3.join("\n")
        );
        let synthesis_prompt = SYNTHESIS_PROMPT.replace("<<EXTRACTIONS>>", &extraction_text);
        let synthesis_response = self.chat(&synthesis_prompt, 1).await?;
        let mut tokens = first.tokens + second.tokens + third.tokens + synthesis_response.tokens;
        let synthesized_notes = clean_bulleted_output(&synthesis_response.content);
        let final_notes = if synthesized_notes.is_empty() {
            merge_unique_notes([&notes_1, &notes_2, &notes_3])
        } else {
            synthesized_notes
        };
        let topics_result = self.generate_topics(&final_notes.join("\n")).await?;
        tokens += topics_result.tokens;
        let relevancies: Vec<usize> = [
            first.page.summary_relevancy,
            second.page.summary_relevancy,
            third.page.summary_relevancy,
        ]
        .into_iter()
        .flatten()
        .map(usize::from)
        .collect();

        let mut page = page.clone();
        page.summary_notes = if final_notes.is_empty() {
            None
        } else {
            Some(final_notes)
        };
        page.summary_notes_1 = if notes_1.is_empty() {
            None
        } else {
            Some(notes_1)
        };
        page.summary_notes_2 = if notes_2.is_empty() {
            None
        } else {
            Some(notes_2)
        };
        page.summary_notes_3 = if notes_3.is_empty() {
            None
        } else {
            Some(notes_3)
        };
        page.summary_topics = topics_result.topics;
        page.summary_relevancy = average_percent(&relevancies).or(Some(0));
        page.summary_quality_validated = Some(
            [
                first.page.summary_quality_validated,
                second.page.summary_quality_validated,
                third.page.summary_quality_validated,
            ]
            .into_iter()
            .all(|validated| validated == Some(true)),
        );

        Ok(SummarizationResult {
            page,
            attempts_used: first.attempts_used + second.attempts_used + third.attempts_used + 1,
            tokens,
            budget_exhausted: first
                .budget_exhausted
                .or(second.budget_exhausted)
                .or(third.budget_exhausted),
        })
    }

    async fn summarize_insight(
        &self,
        page: &PageOutput,
    ) -> Result<SummarizationResult, PipelineError> {
        let mut standard = self.summarize_full(page).await?;
        let notes = standard.page.summary_notes.clone().unwrap_or_default();
        let insight_prompt = INSIGHT_PROMPT
            .replace("<<CONTEXT>>", &page_context(page))
            .replace("<<NOTES>>", &notes.join("\n"));
        let insight_response = self.chat(&insight_prompt, 1).await?;
        standard.tokens += insight_response.tokens;
        standard.attempts_used += 1;

        let insights = clean_bulleted_output(&insight_response.content);
        if !insights.is_empty() {
            let mut combined = notes;
            combined.extend(insights);
            standard.page.summary_notes = Some(combined);
        }
        Ok(standard)
    }

    async fn generate_topics(&self, notes_text: &str) -> Result<TopicsResult, PipelineError> {
        if notes_text.trim().is_empty() {
            return Ok(TopicsResult {
                topics: None,
                tokens: 0,
            });
        }

        let prompt = TOPICS_PROMPT.replace("<<NOTES>>", notes_text);
        let mut tokens = 0;
        for attempt in 0..3 {
            let tier = (attempt + 1).min(3) as u8;
            let response = self.chat(&prompt, tier).await?;
            tokens += response.tokens;
            let topics = clean_topics_output(&response.content);
            if !topics.is_empty() {
                return Ok(TopicsResult {
                    topics: Some(topics),
                    tokens,
                });
            }
        }

        Ok(TopicsResult {
            topics: None,
            tokens,
        })
    }
}

#[async_trait]
impl Summarizer for OpenAiCompatibleSummarizer {
    async fn summarize_page(
        &self,
        page: &PageOutput,
        options: SummarizationOptions,
    ) -> Result<SummarizationResult, PipelineError> {
        if options.insight_mode && options.mode == SummarizerMode::Full {
            return self.summarize_insight(page).await;
        }
        if options.detailed_extraction && options.mode == SummarizerMode::Full {
            return self.summarize_detailed(page).await;
        }

        match options.mode {
            SummarizerMode::Skip => {
                let mut page = page.clone();
                page.summary_notes = None;
                page.summary_topics = None;
                page.summary_relevancy = Some(0);
                page.summary_quality_validated = None;
                Ok(SummarizationResult {
                    page,
                    attempts_used: 0,
                    tokens: 0,
                    budget_exhausted: None,
                })
            }
            SummarizerMode::TopicsOnly => self.summarize_topics_only(page).await,
            SummarizerMode::Full => self.summarize_full(page).await,
        }
    }
}

#[derive(Debug, Clone)]
pub struct CliSummarizer {
    executable: String,
    args: Vec<String>,
    timeout_seconds: u64,
    retries: u32,
    kind: CliProviderKind,
    reported_model: Arc<Mutex<Option<String>>>,
}

impl CliSummarizer {
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

    async fn execute_prompt(&self, prompt: &str) -> Result<String, PipelineError> {
        if self.kind == CliProviderKind::Codex {
            return self.execute_codex_prompt(prompt).await;
        }
        if self.kind == CliProviderKind::Grok {
            return self.execute_grok_prompt(prompt).await;
        }
        if self.kind == CliProviderKind::Copilot {
            return self.execute_copilot_prompt(prompt).await;
        }

        let context = cli_command_context(
            "CLI summarizer",
            &self.executable,
            &self.args,
            self.timeout_seconds,
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
            prompt,
            &context,
            self.timeout_seconds,
            "CLI summarizer",
            self.retry_policy(),
        )
        .await
        .map_err(PipelineError::Summarization)?;

        Ok(clean_response_content(&output.stdout))
    }

    async fn execute_codex_prompt(&self, prompt: &str) -> Result<String, PipelineError> {
        let temp_dir = tempfile::tempdir().map_err(|err| {
            PipelineError::Summarization(format!("could not create Codex temp dir: {err}"))
        })?;
        let mut args = vec![
            "exec".to_string(),
            "-C".to_string(),
            temp_dir.path().display().to_string(),
            "-s".to_string(),
            "read-only".to_string(),
            "--skip-git-repo-check".to_string(),
            "--json".to_string(),
        ];
        args.extend(self.args.clone());
        args.push("-".to_string());
        let context = cli_command_context(
            "Codex CLI summarizer",
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
            "CLI summarizer",
            self.retry_policy(),
        )
        .await
        .map_err(PipelineError::Summarization)?;
        let parsed = parse_codex_jsonl_output(&output.stdout);
        if let Some(model) = parsed.model {
            if let Ok(mut reported_model) = self.reported_model.lock() {
                *reported_model = Some(model);
            }
        }
        if parsed.content.trim().is_empty() {
            return Err(PipelineError::Summarization(format!(
                "Codex CLI returned no assistant message; {context}; stdout={}; stderr={}",
                output.stdout.trim(),
                output.stderr.trim()
            )));
        }
        Ok(clean_response_content(&parsed.content))
    }

    async fn execute_grok_prompt(&self, prompt: &str) -> Result<String, PipelineError> {
        let temp_dir = tempfile::tempdir().map_err(|err| {
            PipelineError::Summarization(format!("could not create Grok temp dir: {err}"))
        })?;
        let prompt_path = temp_dir.path().join("prompt.txt");
        tokio::fs::write(&prompt_path, prompt)
            .await
            .map_err(|err| {
                PipelineError::Summarization(format!("could not write Grok prompt file: {err}"))
            })?;
        let grok_home = create_isolated_grok_home(temp_dir.path())
            .await
            .map_err(PipelineError::Summarization)?;

        let mut args = self.args.clone();
        args.extend([
            "--cwd".to_string(),
            temp_dir.path().display().to_string(),
            "--output-format".to_string(),
            "json".to_string(),
            "--prompt-file".to_string(),
            prompt_path.display().to_string(),
        ]);
        let context = cli_command_context(
            "Grok CLI summarizer",
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
            "CLI summarizer",
            self.retry_policy(),
        )
        .await
        .map_err(PipelineError::Summarization)?;
        let content = parse_grok_json(&output.stdout);
        if content.trim().is_empty() {
            return Err(PipelineError::Summarization(format!(
                "Grok CLI returned no assistant text; {context}; stdout={}; stderr={}",
                output.stdout.trim(),
                output.stderr.trim()
            )));
        }
        Ok(clean_response_content(&content))
    }

    async fn execute_copilot_prompt(&self, prompt: &str) -> Result<String, PipelineError> {
        let mut args = vec![
            "-p".to_string(),
            prompt.to_string(),
            "--allow-all-tools".to_string(),
            "--no-color".to_string(),
            "-s".to_string(),
        ];
        args.extend(self.args.clone());
        let context = cli_command_context(
            "Copilot CLI summarizer",
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
            "CLI summarizer",
            self.retry_policy(),
        )
        .await
        .map_err(PipelineError::Summarization)?;
        let content = clean_response_content(&output.stdout);
        if content.trim().is_empty() {
            return Err(PipelineError::Summarization(format!(
                "Copilot CLI returned empty output; {context}; stderr={}",
                output.stderr.trim()
            )));
        }
        Ok(content)
    }

    async fn summarize_full(
        &self,
        page: &PageOutput,
    ) -> Result<SummarizationResult, PipelineError> {
        let context = page_context(page);
        let notes_prompt = render_notes_prompt(SummaryPrompt::Primary, &context);
        let notes_output = self.execute_prompt(&notes_prompt).await?;
        let notes = clean_bulleted_output(&notes_output);
        let topics = if notes.is_empty() {
            None
        } else {
            let topics_prompt = TOPICS_PROMPT.replace("<<NOTES>>", &notes.join("\n"));
            let topics_output = self.execute_prompt(&topics_prompt).await?;
            let topics = clean_topics_output(&topics_output);
            if topics.is_empty() {
                None
            } else {
                Some(topics)
            }
        };

        let mut page = page.clone();
        page.summary_notes = if notes.is_empty() { None } else { Some(notes) };
        page.summary_topics = topics;
        page.summary_relevancy = None;
        page.summary_quality_validated = page.summary_notes.as_ref().map(|_| false);
        Ok(SummarizationResult {
            page,
            attempts_used: 1,
            tokens: 0,
            budget_exhausted: None,
        })
    }

    async fn summarize_topics_only(
        &self,
        page: &PageOutput,
    ) -> Result<SummarizationResult, PipelineError> {
        let topics_prompt = TOPICS_PROMPT.replace("<<NOTES>>", &page_context(page));
        let topics_output = self.execute_prompt(&topics_prompt).await?;
        let topics = clean_topics_output(&topics_output);
        let mut page = page.clone();
        page.summary_notes = None;
        page.summary_topics = if topics.is_empty() {
            None
        } else {
            Some(topics)
        };
        page.summary_relevancy = None;
        page.summary_quality_validated = None;
        Ok(SummarizationResult {
            page,
            attempts_used: 1,
            tokens: 0,
            budget_exhausted: None,
        })
    }
}

fn resolved_cli_executable_value(executable: &str) -> String {
    resolve_cli_executable(executable)
        .map(|path| path.display().to_string())
        .unwrap_or_else(|| executable.trim().to_string())
}

#[async_trait]
impl Summarizer for CliSummarizer {
    async fn summarize_page(
        &self,
        page: &PageOutput,
        options: SummarizationOptions,
    ) -> Result<SummarizationResult, PipelineError> {
        match options.mode {
            SummarizerMode::Skip => {
                let mut page = page.clone();
                page.summary_notes = None;
                page.summary_topics = None;
                page.summary_relevancy = Some(0);
                page.summary_quality_validated = None;
                Ok(SummarizationResult {
                    page,
                    attempts_used: 0,
                    tokens: 0,
                    budget_exhausted: None,
                })
            }
            SummarizerMode::TopicsOnly => self.summarize_topics_only(page).await,
            SummarizerMode::Full => self.summarize_full(page).await,
        }
    }

    fn reported_model(&self) -> Option<String> {
        self.reported_model.lock().ok()?.clone()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct LlmResponse {
    content: String,
    tokens: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct TopicsResult {
    topics: Option<Vec<String>>,
    tokens: usize,
}

#[derive(Debug, Deserialize)]
struct ChatCompletionResponse {
    choices: Vec<ChatChoice>,
    #[serde(default)]
    usage: Option<ChatUsage>,
}

#[derive(Debug, Deserialize)]
struct ChatChoice {
    message: ChatMessage,
}

#[derive(Debug, Deserialize)]
struct ChatMessage {
    content: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ChatUsage {
    total_tokens: usize,
}

fn page_context(page: &PageOutput) -> String {
    let mut parts = Vec::new();
    if !page.text.trim().is_empty() {
        parts.push(page.text.trim().to_string());
    }
    if !page.tables.is_empty() {
        for (index, table) in page.tables.iter().enumerate() {
            let rows = table
                .iter()
                .map(|row| row.join(" | "))
                .collect::<Vec<_>>()
                .join("\n");
            parts.push(format!("Table {}:\n{}", index + 1, rows));
        }
    }
    if let Some(image_text) = page
        .image_text
        .as_deref()
        .filter(|text| !text.trim().is_empty())
    {
        parts.push(format!("Image extraction:\n{}", image_text.trim()));
    }
    parts.join("\n\n")
}

fn render_notes_prompt(prompt: SummaryPrompt, chunk: &str) -> String {
    match prompt {
        SummaryPrompt::Primary => NOTES_PROMPT,
        SummaryPrompt::Fallback => NOTES_FALLBACK_PROMPT,
    }
    .replace("{chunk}", chunk)
}

fn render_quality_prompt(text: &str, summary: &str) -> String {
    format!(
        "Compare the following original text and summary.\n\n\
         On a scale from 0% to 100%, how accurately does the summary represent the original text?\n\n\
         Original Text:\n\"\"\"{text}\"\"\"\n\n\
         Summary:\n\"\"\"{summary}\"\"\"\n\n\
         Provide only the percentage number."
    )
}

fn clean_response_content(content: &str) -> String {
    content
        .trim()
        .trim_start_matches("<|channel|>thought<channel|>")
        .trim()
        .to_string()
}

fn clean_bulleted_output(content: &str) -> Vec<String> {
    content
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(|line| {
            line.trim_start_matches(['*', '-', '•'])
                .trim_start_matches(|ch: char| ch.is_ascii_digit() || ch == '.' || ch == ')')
                .trim()
        })
        .filter(|line| !line.is_empty())
        .map(ToString::to_string)
        .collect()
}

fn clean_topics_output(content: &str) -> Vec<String> {
    content
        .lines()
        .flat_map(|line| line.split(','))
        .map(|topic| {
            topic
                .trim()
                .trim_start_matches(['*', '-', '•'])
                .trim_start_matches(|ch: char| ch.is_ascii_digit() || ch == '.' || ch == ')')
                .trim()
        })
        .filter(|topic| !topic.is_empty())
        .take(5)
        .map(ToString::to_string)
        .collect()
}

fn parse_percent(content: &str) -> Option<u8> {
    let digits: String = content
        .chars()
        .skip_while(|ch| !ch.is_ascii_digit())
        .take_while(|ch| ch.is_ascii_digit())
        .collect();
    let value = digits.parse::<u16>().ok()?;
    u8::try_from(value).ok().filter(|value| *value <= 100)
}

fn merge_unique_notes<'a>(groups: impl IntoIterator<Item = &'a Vec<String>>) -> Vec<String> {
    let mut merged = Vec::new();
    for note in groups.into_iter().flatten() {
        if !merged.contains(note) {
            merged.push(note.clone());
        }
    }
    merged
}

fn average_percent(values: &[usize]) -> Option<u8> {
    if values.is_empty() {
        None
    } else {
        Some((values.iter().sum::<usize>() / values.len()).min(100) as u8)
    }
}

fn summarizer_http_client(timeout: Duration) -> Client {
    Client::builder()
        .connect_timeout(Duration::from_secs(10))
        .timeout(timeout)
        .build()
        .expect("summarizer HTTP client configuration should be valid")
}

fn summarizer_http_error(err: reqwest::Error) -> PipelineError {
    if err.is_timeout() {
        PipelineError::Summarization(format!("summarizer provider request timed out: {err}"))
    } else {
        PipelineError::Summarization(err.to_string())
    }
}

async fn summarizer_json_response<T: DeserializeOwned>(
    response: Response,
    provider_name: &str,
) -> Result<T, PipelineError> {
    let status = response.status();
    if !status.is_success() {
        let body = read_summarizer_error_body(response).await?;
        return Err(PipelineError::Summarization(status_error_message(
            provider_name,
            status,
            &body,
        )));
    }

    response
        .json()
        .await
        .map_err(|err| PipelineError::Summarization(err.to_string()))
}

async fn read_summarizer_error_body(mut response: Response) -> Result<String, PipelineError> {
    let mut body = Vec::new();
    let mut truncated = false;

    while let Some(chunk) = response.chunk().await.map_err(summarizer_http_error)? {
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
        clean_bulleted_output, clean_topics_output, cli_command_context, parse_percent,
        QualityPolicy, SummaryPrompt,
    };
    use summarizer_cli_util::unavailable_output;

    #[test]
    fn quality_policy_matches_spec() {
        let policy = QualityPolicy::default();

        assert_eq!(policy.threshold_for_attempt(1), 90);
        assert_eq!(policy.threshold_for_attempt(5), 90);
        assert_eq!(policy.threshold_for_attempt(6), 85);
        assert_eq!(policy.threshold_for_attempt(30), 85);
        assert_eq!(policy.tier_for_attempt(1), 1);
        assert_eq!(policy.tier_for_attempt(11), 2);
        assert_eq!(policy.tier_for_attempt(21), 3);
        assert_eq!(policy.prompt_for_attempt(6), SummaryPrompt::Fallback);
    }

    #[test]
    fn parsers_clean_common_llm_outputs() {
        assert_eq!(
            clean_bulleted_output("* First point\n- Second point\n3. Third point"),
            ["First point", "Second point", "Third point"]
        );
        assert_eq!(
            clean_topics_output("Alpha, Beta\n* Gamma"),
            ["Alpha", "Beta", "Gamma"]
        );
        assert_eq!(parse_percent("92%"), Some(92));
        assert_eq!(parse_percent("8/10"), Some(8));
        assert_eq!(parse_percent("92 out of 100"), Some(92));
        assert_eq!(parse_percent("0.85"), Some(0));
        assert_eq!(parse_percent("score: 110"), None);
        assert_eq!(parse_percent("no score"), None);
    }

    #[test]
    fn cli_error_context_names_command_args_timeout_and_streams() {
        let args = vec!["--model".to_string(), "test-model".to_string()];
        let context = cli_command_context("CLI summarizer", "/usr/bin/example", &args, 42);
        assert!(context.contains("executable=/usr/bin/example"));
        assert!(context.contains("--model"));
        assert!(context.contains("test-model"));
        assert!(context.contains("timeout_seconds=42"));

        let output = unavailable_output("process did not start");
        assert!(output.contains("stdout="));
        assert!(output.contains("stderr="));
    }
}
