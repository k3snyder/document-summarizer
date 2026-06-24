use chrono::{DateTime, Utc};
use serde::{Deserialize, Deserializer, Serialize};
use std::collections::BTreeMap;
use thiserror::Error;
use utoipa::ToSchema;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum JobState {
    Pending,
    Processing,
    Completed,
    Failed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct JobStatus {
    pub job_id: String,
    pub status: JobState,
    pub progress: u8,
    pub current_stage: Option<String>,
    pub message: Option<String>,
    pub file_name: String,
    pub created_at: DateTime<Utc>,
    pub started_at: Option<DateTime<Utc>>,
    pub completed_at: Option<DateTime<Utc>>,
    pub error: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct JobCreateResponse {
    pub job_id: String,
    pub status: JobState,
    pub message: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub enum PdfImageDpi {
    #[serde(rename = "72")]
    Dpi72,
    #[serde(rename = "144")]
    Dpi144,
    #[serde(rename = "200")]
    Dpi200,
    #[serde(rename = "300")]
    Dpi300,
}

impl Default for PdfImageDpi {
    fn default() -> Self {
        Self::Dpi200
    }
}

impl PdfImageDpi {
    pub fn as_u16(self) -> u16 {
        match self {
            Self::Dpi72 => 72,
            Self::Dpi144 => 144,
            Self::Dpi200 => 200,
            Self::Dpi300 => 300,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum VisionMode {
    None,
    Deepseek,
    Gemini,
    Openai,
    Ollama,
    LlamaCpp,
    Codex,
    Claude,
    Grok,
}

impl Default for VisionMode {
    fn default() -> Self {
        Self::None
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "kebab-case")]
pub enum SummarizerMode {
    Full,
    TopicsOnly,
    Skip,
}

impl Default for SummarizerMode {
    fn default() -> Self {
        Self::Full
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum SummarizerProvider {
    Ollama,
    LlamaCpp,
    Openai,
    Codex,
    Claude,
    Grok,
}

impl Default for SummarizerProvider {
    fn default() -> Self {
        Self::LlamaCpp
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum CliProvider {
    Codex,
    Claude,
    Grok,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct PipelineConfig {
    #[serde(default = "default_true")]
    pub run_extraction: bool,
    #[serde(default)]
    pub extract_only: bool,
    #[serde(default)]
    pub skip_tables: bool,
    #[serde(default)]
    pub skip_images: bool,
    #[serde(default)]
    pub skip_pptx_tables: bool,
    #[serde(default)]
    pub text_only: bool,
    #[serde(default, deserialize_with = "deserialize_pdf_image_dpi")]
    pub pdf_image_dpi: PdfImageDpi,
    #[serde(default)]
    pub vision_mode: VisionMode,
    #[serde(default)]
    pub vision_classifier_mode: Option<VisionMode>,
    #[serde(default)]
    pub vision_extractor_mode: Option<VisionMode>,
    #[serde(default)]
    pub vision_cli_provider: Option<CliProvider>,
    #[serde(default)]
    pub vision_skip_classification: bool,
    /// Deprecated and ignored. Accepted only so older settings/API payloads keep loading.
    #[serde(default, skip_serializing)]
    pub vision_detailed_extraction: bool,
    #[serde(default = "default_chunk_size")]
    pub chunk_size: usize,
    #[serde(default = "default_chunk_overlap")]
    pub chunk_overlap: usize,
    #[serde(default = "default_true")]
    pub run_summarization: bool,
    #[serde(default)]
    pub summarizer_mode: SummarizerMode,
    #[serde(default)]
    pub summarizer_provider: SummarizerProvider,
    #[serde(default)]
    pub summarizer_detailed_extraction: bool,
    #[serde(default)]
    pub summarizer_insight_mode: bool,
    #[serde(default)]
    pub summarizer_cli_provider: Option<CliProvider>,
    #[serde(default = "default_max_tokens_per_page")]
    pub max_tokens_per_page: usize,
    #[serde(default = "default_max_seconds_per_page")]
    pub max_seconds_per_page: u64,
    #[serde(default)]
    pub keep_base64_images: bool,
}

impl Default for PipelineConfig {
    fn default() -> Self {
        Self {
            run_extraction: true,
            extract_only: false,
            skip_tables: false,
            skip_images: false,
            skip_pptx_tables: false,
            text_only: false,
            pdf_image_dpi: PdfImageDpi::Dpi200,
            vision_mode: VisionMode::None,
            vision_classifier_mode: None,
            vision_extractor_mode: None,
            vision_cli_provider: None,
            vision_skip_classification: false,
            vision_detailed_extraction: false,
            chunk_size: default_chunk_size(),
            chunk_overlap: default_chunk_overlap(),
            run_summarization: true,
            summarizer_mode: SummarizerMode::Full,
            summarizer_provider: SummarizerProvider::LlamaCpp,
            summarizer_detailed_extraction: false,
            summarizer_insight_mode: false,
            summarizer_cli_provider: None,
            max_tokens_per_page: default_max_tokens_per_page(),
            max_seconds_per_page: default_max_seconds_per_page(),
            keep_base64_images: false,
        }
    }
}

fn default_true() -> bool {
    true
}

fn default_chunk_size() -> usize {
    3000
}

fn default_chunk_overlap() -> usize {
    80
}

pub fn default_max_tokens_per_page() -> usize {
    100_000
}

pub fn default_max_seconds_per_page() -> u64 {
    300
}

fn deserialize_pdf_image_dpi<'de, D>(deserializer: D) -> Result<PdfImageDpi, D::Error>
where
    D: Deserializer<'de>,
{
    let value = serde_json::Value::deserialize(deserializer)?;
    match value {
        serde_json::Value::Number(number) => match number.as_u64() {
            Some(72) => Ok(PdfImageDpi::Dpi72),
            Some(144) => Ok(PdfImageDpi::Dpi144),
            Some(200) => Ok(PdfImageDpi::Dpi200),
            Some(300) => Ok(PdfImageDpi::Dpi300),
            Some(other) => Err(serde::de::Error::custom(format!(
                "unsupported pdf_image_dpi value {other}"
            ))),
            None => Err(serde::de::Error::custom(
                "pdf_image_dpi must be a positive integer",
            )),
        },
        serde_json::Value::String(value) => match value.as_str() {
            "72" => Ok(PdfImageDpi::Dpi72),
            "144" => Ok(PdfImageDpi::Dpi144),
            "200" => Ok(PdfImageDpi::Dpi200),
            "300" => Ok(PdfImageDpi::Dpi300),
            other => Err(serde::de::Error::custom(format!(
                "unsupported pdf_image_dpi value {other}"
            ))),
        },
        other => Err(serde::de::Error::custom(format!(
            "pdf_image_dpi must be a number or string, got {other}"
        ))),
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct DocumentMetadata {
    pub document_id: String,
    pub filename: String,
    pub total_pages: usize,
    #[serde(default)]
    pub metadata: BTreeMap<String, serde_json::Value>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct ExtractedImage {
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub relationship_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content_type: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub filename: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub alt_text: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base64: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct TableCell {
    pub text: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub row_span: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub col_span: Option<usize>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub metadata: BTreeMap<String, serde_json::Value>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct PageOutput {
    pub chunk_id: String,
    pub doc_title: String,
    #[serde(default)]
    pub page_number: Option<usize>,
    pub text: String,
    #[serde(default)]
    pub tables: Vec<Vec<Vec<String>>>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub extraction_warnings: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub html: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub embedded_images: Vec<ExtractedImage>,
    #[serde(default)]
    pub image_base64: Option<String>,
    #[serde(default)]
    pub image_text: Option<String>,
    #[serde(default)]
    pub image_classifier: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub image_text_1: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub image_text_2: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub image_text_3: Option<String>,
    #[serde(default)]
    pub summary_notes: Option<Vec<String>>,
    #[serde(default)]
    pub summary_topics: Option<Vec<String>>,
    #[serde(default, deserialize_with = "deserialize_optional_percent")]
    pub summary_relevancy: Option<u8>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summary_quality_validated: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summary_notes_1: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summary_notes_2: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summary_notes_3: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summary_budget_exhausted: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct StageMetrics {
    pub duration_ms: u64,
    pub pages_processed: usize,
    pub tokens: usize,
    #[serde(default)]
    pub pages_with_images: Option<usize>,
    #[serde(default)]
    pub classified_count: Option<usize>,
    #[serde(default)]
    pub extracted_count: Option<usize>,
    #[serde(default, deserialize_with = "deserialize_optional_percent")]
    pub avg_relevancy: Option<u8>,
    #[serde(default)]
    pub total_attempts: Option<usize>,
}

impl StageMetrics {
    pub fn empty() -> Self {
        Self {
            duration_ms: 0,
            pages_processed: 0,
            tokens: 0,
            pages_with_images: None,
            classified_count: None,
            extracted_count: None,
            avg_relevancy: None,
            total_attempts: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct PipelineMetricsConfig {
    pub vision_mode: Option<String>,
    #[serde(default)]
    pub vision_classifier_provider: Option<String>,
    #[serde(default)]
    pub vision_extractor_provider: Option<String>,
    pub summarizer_provider: Option<String>,
    pub summarizer_mode: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct PipelineStages {
    pub extraction: StageMetrics,
    pub vision: StageMetrics,
    pub summarization: StageMetrics,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct PipelineMetrics {
    pub total_duration_ms: u64,
    pub total_tokens: usize,
    pub stages: PipelineStages,
    pub config: PipelineMetricsConfig,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct DocumentOutput {
    pub document: DocumentMetadata,
    pub pages: Vec<PageOutput>,
    #[serde(default)]
    pub metrics: Option<PipelineMetrics>,
}

impl DocumentOutput {
    pub fn to_markdown(&self) -> String {
        let mut lines = vec![format!("# {}", self.document.filename), String::new()];
        let has_any_summary = self.pages.iter().any(page_has_summary);

        for (index, page) in self.pages.iter().enumerate() {
            let has_notes = page_has_notes(page);
            let has_topics = page_has_topics(page);
            if !has_notes && !has_topics && has_any_summary {
                continue;
            }

            lines.push(format!("## Page {}", page.page_number.unwrap_or(index + 1)));
            lines.push(String::new());

            if let Some(topics) = &page.summary_topics {
                if !topics.is_empty() {
                    lines.push("### Topics".to_string());
                    lines.extend(topics.iter().map(|topic| format!("- {topic}")));
                    lines.push(String::new());
                }
            }

            if let Some(notes) = &page.summary_notes {
                if !notes.is_empty() {
                    lines.push("### Summary Notes".to_string());
                    lines.extend(notes.iter().map(|note| format!("- {note}")));
                    lines.push(String::new());
                }
            }

            if !has_any_summary {
                if !page.text.trim().is_empty() {
                    lines.push("### Extracted Text".to_string());
                    lines.push(page.text.trim().to_string());
                    lines.push(String::new());
                }

                if !page.tables.is_empty() {
                    lines.push("### Tables".to_string());
                    for (table_index, table) in page.tables.iter().enumerate() {
                        if page.tables.len() > 1 {
                            lines.push(format!("#### Table {}", table_index + 1));
                        }
                        lines.extend(table_to_markdown(table));
                        lines.push(String::new());
                    }
                }
            }
        }

        lines.join("\n")
    }
}

fn page_has_summary(page: &PageOutput) -> bool {
    page_has_notes(page) || page_has_topics(page)
}

fn page_has_notes(page: &PageOutput) -> bool {
    page.summary_notes
        .as_ref()
        .is_some_and(|notes| !notes.is_empty())
}

fn page_has_topics(page: &PageOutput) -> bool {
    page.summary_topics
        .as_ref()
        .is_some_and(|topics| !topics.is_empty())
}

fn table_to_markdown(table: &[Vec<String>]) -> Vec<String> {
    if table.is_empty() {
        return Vec::new();
    }

    let column_count = table.iter().map(Vec::len).max().unwrap_or(0);
    if column_count == 0 {
        return Vec::new();
    }

    let mut lines = Vec::new();
    for (row_index, row) in table.iter().enumerate() {
        let mut cells = row.clone();
        cells.resize(column_count, String::new());
        lines.push(format!("| {} |", cells.join(" | ")));
        if row_index == 0 {
            lines.push(format!("|{}|", vec!["---"; column_count].join("|")));
        }
    }
    lines
}

#[derive(Debug, Error)]
pub enum PipelineError {
    #[error("invalid config: {0}")]
    Config(String),
    #[error("unsupported file type: {0}")]
    UnsupportedFileType(String),
    #[error("extraction failed: {0}")]
    Extraction(String),
    #[error("vision failed: {0}")]
    Vision(String),
    #[error("summarization failed: {0}")]
    Summarization(String),
    #[error("storage failed: {0}")]
    Storage(String),
    #[error("database failed: {0}")]
    Database(String),
    #[error("request failed: {0}")]
    Request(String),
    #[error("pipeline failed: {0}")]
    Pipeline(String),
}

fn deserialize_optional_percent<'de, D>(deserializer: D) -> Result<Option<u8>, D::Error>
where
    D: Deserializer<'de>,
{
    let value = Option::<serde_json::Value>::deserialize(deserializer)?;
    let Some(value) = value else {
        return Ok(None);
    };

    let number = match value {
        serde_json::Value::Number(number) => number
            .as_f64()
            .ok_or_else(|| serde::de::Error::custom("percentage must be numeric"))?,
        other => {
            return Err(serde::de::Error::custom(format!(
                "percentage must be numeric, got {other}"
            )))
        }
    };

    if !(0.0..=100.0).contains(&number) {
        return Err(serde::de::Error::custom(
            "percentage must be between 0 and 100",
        ));
    }

    Ok(Some(number.round() as u8))
}
