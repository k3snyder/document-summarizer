use base64::{engine::general_purpose, Engine as _};
use image::ImageFormat;
use pdfium_render::prelude::{PdfRenderConfig, Pdfium};
use std::{
    collections::HashMap,
    fs::{self, File},
    io::{Cursor, Read},
    ops::Deref,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::{Arc, Mutex, MutexGuard, OnceLock},
};
use summarizer_cli_util::{resolve_soffice, suppress_command_window};
use summarizer_types::{
    DocumentMetadata, DocumentOutput, PageOutput, PipelineConfig, PipelineError, VisionMode,
};
use url::Url;
use zip::ZipArchive;

mod docx;
mod opc;

const MAX_DECOMPRESSED_ARCHIVE_BYTES: u64 = 500_000_000;
static PDFIUM: OnceLock<Result<Mutex<LockedPdfium>, String>> = OnceLock::new();
static PDFIUM_LIBRARY_PATH: OnceLock<PathBuf> = OnceLock::new();
type SlideTables = Vec<Vec<Vec<String>>>;

struct LockedPdfium(Pdfium);

// Pdfium's Rust bindings are not Send/Sync, but this module keeps the single
// process-wide binding behind a mutex and never exposes it without the guard.
unsafe impl Send for LockedPdfium {}

impl Deref for LockedPdfium {
    type Target = Pdfium;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

pub fn configure_pdfium_library_path(path: impl Into<PathBuf>) -> Result<(), PipelineError> {
    let path = path.into();
    if PDFIUM_LIBRARY_PATH
        .get()
        .is_some_and(|existing| existing == &path)
    {
        return Ok(());
    }
    if PDFIUM.get().is_some() {
        return Err(PipelineError::Extraction(
            "PDFium has already been initialized; configure the library path before processing PDF/PPTX documents"
                .to_string(),
        ));
    }
    PDFIUM_LIBRARY_PATH.set(path).map_err(|_| {
        PipelineError::Extraction(
            "PDFium library path has already been configured for this process".to_string(),
        )
    })
}

struct SizeLimitedZipArchive {
    archive: ZipArchive<File>,
    decompressed_bytes: u64,
    accounted_entries: HashMap<String, u64>,
}

impl SizeLimitedZipArchive {
    fn new(file: File, label: &str) -> Result<Self, PipelineError> {
        let archive = ZipArchive::new(file)
            .map_err(|err| PipelineError::Extraction(format!("Invalid {label} archive: {err}")))?;
        Ok(Self {
            archive,
            decompressed_bytes: 0,
            accounted_entries: HashMap::new(),
        })
    }

    fn len(&self) -> usize {
        self.archive.len()
    }

    fn entry_name(&mut self, index: usize) -> Option<String> {
        self.archive
            .by_index(index)
            .ok()
            .map(|file| file.name().to_string())
    }

    fn read_string(&mut self, path: &str, label: &str) -> Result<String, PipelineError> {
        let contents = self.read_limited_bytes(path, label)?;
        String::from_utf8(contents).map_err(|err| {
            PipelineError::Extraction(format!("Could not decode {label} entry {path}: {err}"))
        })
    }

    fn read_optional_string(
        &mut self,
        path: &str,
        label: &str,
    ) -> Result<Option<String>, PipelineError> {
        let Some(contents) = self.read_optional_limited_bytes(path, label)? else {
            return Ok(None);
        };
        String::from_utf8(contents).map(Some).map_err(|err| {
            PipelineError::Extraction(format!("Could not decode {label} entry {path}: {err}"))
        })
    }

    fn read_bytes(&mut self, path: &str, label: &str) -> Result<Vec<u8>, PipelineError> {
        self.read_limited_bytes(path, label)
    }

    fn read_limited_bytes(&mut self, path: &str, label: &str) -> Result<Vec<u8>, PipelineError> {
        let Some(contents) = self.read_optional_limited_bytes(path, label)? else {
            return Err(PipelineError::Extraction(format!(
                "Missing {label} entry {path}: file not found"
            )));
        };
        Ok(contents)
    }

    fn read_optional_limited_bytes(
        &mut self,
        path: &str,
        label: &str,
    ) -> Result<Option<Vec<u8>>, PipelineError> {
        let declared_size = {
            match self.archive.by_name(path) {
                Ok(file) => file.size(),
                Err(zip::result::ZipError::FileNotFound) => return Ok(None),
                Err(err) => {
                    return Err(PipelineError::Extraction(format!(
                        "Could not open {label} entry {path}: {err}"
                    )));
                }
            }
        };
        let limit = declared_size.checked_add(1).ok_or_else(|| {
            PipelineError::Extraction(format!(
                "{label} entry {path} exceeds decompressed size limit"
            ))
        })?;
        self.ensure_declared_size_can_be_read(declared_size, label, path)?;
        let contents = {
            let mut file = self.archive.by_name(path).map_err(|err| {
                PipelineError::Extraction(format!("Could not open {label} entry {path}: {err}"))
            })?;
            let mut contents = Vec::new();
            file.by_ref()
                .take(limit)
                .read_to_end(&mut contents)
                .map_err(|err| {
                    PipelineError::Extraction(format!("Could not read {label} entry {path}: {err}"))
                })?;
            contents
        };
        if contents.len() as u64 > declared_size {
            return Err(PipelineError::Extraction(format!(
                "{label} entry {path} inflated beyond declared size"
            )));
        }
        self.account_entry(path, contents.len() as u64, label)?;
        Ok(Some(contents))
    }

    fn ensure_declared_size_can_be_read(
        &self,
        size: u64,
        label: &str,
        path: &str,
    ) -> Result<(), PipelineError> {
        if size > MAX_DECOMPRESSED_ARCHIVE_BYTES {
            return Err(PipelineError::Extraction(format!(
                "{label} entry {path} exceeds decompressed size limit"
            )));
        }
        Ok(())
    }

    fn account_entry(&mut self, path: &str, size: u64, label: &str) -> Result<(), PipelineError> {
        if self.accounted_entries.contains_key(path) {
            return Ok(());
        }
        let next_total = self.decompressed_bytes.checked_add(size).ok_or_else(|| {
            PipelineError::Extraction(format!(
                "{label} entry {path} exceeds decompressed size limit"
            ))
        })?;
        if next_total > MAX_DECOMPRESSED_ARCHIVE_BYTES {
            return Err(PipelineError::Extraction(format!(
                "{label} archive exceeds decompressed size limit of {MAX_DECOMPRESSED_ARCHIVE_BYTES} bytes"
            )));
        }
        self.decompressed_bytes = next_total;
        self.accounted_entries.insert(path.to_string(), size);
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DocumentKind {
    Pdf,
    Pptx,
    Docx,
    Text,
    Markdown,
}

impl DocumentKind {
    pub fn from_path(path: &Path) -> Result<Self, PipelineError> {
        match path
            .extension()
            .and_then(|extension| extension.to_str())
            .map(|extension| extension.to_ascii_lowercase())
            .as_deref()
        {
            Some("pdf") => Ok(Self::Pdf),
            Some("pptx") => Ok(Self::Pptx),
            Some("docx") => Ok(Self::Docx),
            Some("txt") => Ok(Self::Text),
            Some("md") | Some("markdown") => Ok(Self::Markdown),
            Some(extension) => Err(PipelineError::UnsupportedFileType(extension.to_string())),
            None => Err(PipelineError::UnsupportedFileType(
                "missing extension".to_string(),
            )),
        }
    }
}

#[derive(Debug, Clone)]
pub struct Extractor;

#[derive(Debug, Clone)]
pub struct ExtractionProgress {
    pub page_number: usize,
    pub total_pages: usize,
    pub completed_pages: usize,
    pub message: String,
}

pub type ExtractionProgressCallback = Arc<dyn Fn(ExtractionProgress) + Send + Sync>;

impl Extractor {
    pub fn new() -> Self {
        Self
    }

    pub async fn extract_path(
        &self,
        document_id: String,
        path: &Path,
        config: &PipelineConfig,
    ) -> Result<DocumentOutput, PipelineError> {
        self.extract_path_with_progress(document_id, path, config, noop_extraction_progress())
            .await
    }

    pub async fn extract_path_with_progress(
        &self,
        document_id: String,
        path: &Path,
        config: &PipelineConfig,
        progress: ExtractionProgressCallback,
    ) -> Result<DocumentOutput, PipelineError> {
        let filename = path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("document")
            .to_string();

        match DocumentKind::from_path(path)? {
            DocumentKind::Text | DocumentKind::Markdown => {
                let text = tokio::fs::read_to_string(path)
                    .await
                    .map_err(|err| PipelineError::Extraction(err.to_string()))?;
                Ok(extract_text_document_with_progress(
                    document_id,
                    filename,
                    &text,
                    config,
                    progress.as_ref(),
                ))
            }
            DocumentKind::Pdf => {
                let config = config.clone();
                let path = path.to_path_buf();
                let progress = Arc::clone(&progress);
                tokio::task::spawn_blocking(move || {
                    extract_pdf_document_with_progress(
                        document_id,
                        filename,
                        &path,
                        &config,
                        progress.as_ref(),
                    )
                })
                .await
                .map_err(|err| PipelineError::Extraction(err.to_string()))?
            }
            DocumentKind::Pptx => {
                let config = config.clone();
                let path = path.to_path_buf();
                let progress = Arc::clone(&progress);
                tokio::task::spawn_blocking(move || {
                    extract_pptx_document_with_progress(
                        document_id,
                        filename,
                        &path,
                        &config,
                        progress.as_ref(),
                    )
                })
                .await
                .map_err(|err| PipelineError::Extraction(err.to_string()))?
            }
            DocumentKind::Docx => {
                let config = config.clone();
                let path = path.to_path_buf();
                let progress = Arc::clone(&progress);
                tokio::task::spawn_blocking(move || {
                    docx::extract_docx_document_with_progress(
                        document_id,
                        filename,
                        &path,
                        &config,
                        progress.as_ref(),
                    )
                })
                .await
                .map_err(|err| PipelineError::Extraction(err.to_string()))?
            }
        }
    }
}

pub fn extract_pdf_document(
    document_id: String,
    filename: String,
    path: &Path,
    config: &PipelineConfig,
) -> Result<DocumentOutput, PipelineError> {
    let progress = noop_extraction_progress();
    extract_pdf_document_with_progress(document_id, filename, path, config, progress.as_ref())
}

fn extract_pdf_document_with_progress(
    document_id: String,
    filename: String,
    path: &Path,
    config: &PipelineConfig,
    progress: &(dyn Fn(ExtractionProgress) + Send + Sync),
) -> Result<DocumentOutput, PipelineError> {
    let pdfium = lock_pdfium()?;
    let document = pdfium
        .load_pdf_from_file(path, None)
        .map_err(|err| PipelineError::Extraction(format!("Could not open PDF: {err}")))?;
    let total_pages = document.pages().len() as usize;
    let mut pages = Vec::with_capacity(total_pages);

    for (index, page) in document.pages().iter().enumerate() {
        let page_number = index + 1;
        emit_extraction_progress(
            progress,
            page_number,
            total_pages,
            index,
            format!("Extracting page {page_number} of {total_pages}."),
        );
        let (text, text_warning) = extract_text_from_pdf_page(&page);
        let extraction_warnings = text_warning.into_iter().collect::<Vec<_>>();
        let tables = if config.skip_tables || config.text_only {
            Vec::new()
        } else {
            extract_tables_from_text(&text)
        };
        let image_base64 = if config.skip_images || config.text_only {
            None
        } else {
            Some(render_page_png_base64(
                &page,
                config.pdf_image_dpi.as_u16(),
            )?)
        };

        pages.push(PageOutput {
            chunk_id: format!("chunk_{page_number}"),
            doc_title: filename.clone(),
            page_number: Some(page_number),
            text,
            tables,
            extraction_warnings,
            html: None,
            embedded_images: Vec::new(),
            image_base64,
            image_text: None,
            image_classifier: None,
            image_text_1: None,
            image_text_2: None,
            image_text_3: None,
            summary_notes: None,
            summary_topics: None,
            summary_relevancy: None,
            summary_quality_validated: None,
            summary_notes_1: None,
            summary_notes_2: None,
            summary_notes_3: None,
            summary_budget_exhausted: None,
        });
        emit_extraction_progress(
            progress,
            page_number,
            total_pages,
            index + 1,
            format!("Extracted page {page_number} of {total_pages}."),
        );
    }

    Ok(DocumentOutput {
        document: DocumentMetadata {
            document_id,
            filename,
            total_pages,
            metadata: Default::default(),
        },
        pages,
        metrics: None,
    })
}

fn extract_text_from_pdf_page(
    page: &pdfium_render::prelude::PdfPage<'_>,
) -> (String, Option<String>) {
    let Ok(text_page) = page.text() else {
        tracing::warn!(target: "summarizer_extraction", "PDF text extraction failed for page");
        return (String::new(), Some("pdf_text_failed".to_string()));
    };

    (move_leading_pdf_footer_to_end(&text_page.all()), None)
}

fn move_leading_pdf_footer_to_end(text: &str) -> String {
    let lines: Vec<&str> = text.lines().collect();
    let Some(first_content_index) = lines.iter().position(|line| !line.trim().is_empty()) else {
        return String::new();
    };

    if !lines[first_content_index].contains('©') {
        return text.trim().to_string();
    }

    let mut footer = Vec::new();
    let mut index = first_content_index;
    while index < lines.len() {
        let line = lines[index].trim();
        if line.is_empty() {
            index += 1;
            break;
        }
        footer.push(line.to_string());
        index += 1;
    }

    let body = lines[index..]
        .iter()
        .map(|line| line.trim_end())
        .collect::<Vec<_>>()
        .join("\n")
        .trim()
        .to_string();
    if body.is_empty() {
        footer.join("\n")
    } else {
        format!("{}\n{}", body, footer.join("\n"))
    }
}

pub fn extract_tables_from_text(text: &str) -> Vec<Vec<Vec<String>>> {
    let mut tables = Vec::new();
    let mut current = Vec::new();
    let mut expected_columns = None;

    for line in text.lines() {
        let Some(cells) = split_table_row(line) else {
            flush_table(&mut tables, &mut current);
            expected_columns = None;
            continue;
        };

        let column_count = cells.len();
        if column_count < 2 {
            flush_table(&mut tables, &mut current);
            expected_columns = None;
            continue;
        }

        if expected_columns.is_some_and(|expected| expected != column_count) {
            flush_table(&mut tables, &mut current);
        }
        expected_columns = Some(column_count);
        current.push(cells);
    }

    flush_table(&mut tables, &mut current);
    tables
}

fn split_table_row(line: &str) -> Option<Vec<String>> {
    let trimmed = line.trim();
    if trimmed.is_empty() {
        return None;
    }

    let cells: Vec<String> = if trimmed.contains('\t') {
        trimmed
            .split('\t')
            .map(str::trim)
            .filter(|cell| !cell.is_empty())
            .map(ToString::to_string)
            .collect()
    } else if trimmed.contains('|') {
        trimmed
            .trim_matches('|')
            .split('|')
            .map(str::trim)
            .filter(|cell| !cell.is_empty())
            .map(ToString::to_string)
            .collect()
    } else {
        split_on_repeated_spaces(trimmed)
    };

    if cells.len() >= 2 {
        Some(cells)
    } else {
        None
    }
}

fn split_on_repeated_spaces(line: &str) -> Vec<String> {
    let mut cells = Vec::new();
    let mut cell = String::new();
    let mut space_count = 0;

    for ch in line.chars() {
        if ch == ' ' {
            space_count += 1;
            if space_count < 2 {
                cell.push(ch);
            }
            continue;
        }

        if space_count >= 2 && !cell.trim().is_empty() {
            cells.push(cell.trim().to_string());
            cell.clear();
        }
        if space_count == 1 && cell.ends_with(' ') {
            cell.pop();
        }
        space_count = 0;
        cell.push(ch);
    }

    if !cell.trim().is_empty() {
        cells.push(cell.trim().to_string());
    }
    cells
}

fn flush_table(tables: &mut Vec<Vec<Vec<String>>>, current: &mut Vec<Vec<String>>) {
    if current.len() >= 2 {
        tables.push(std::mem::take(current));
    } else {
        current.clear();
    }
}

fn render_page_png_base64(
    page: &pdfium_render::prelude::PdfPage<'_>,
    dpi: u16,
) -> Result<String, PipelineError> {
    let target_width = ((page.width().value / 72.0) * f32::from(dpi)).round() as i32;
    let target_width = target_width.clamp(1, 4096);
    let render_config = PdfRenderConfig::new()
        .set_target_width(target_width)
        .render_form_data(true);
    let image = page
        .render_with_config(&render_config)
        .map_err(|err| PipelineError::Extraction(format!("Could not render PDF page: {err}")))?
        .as_image();
    let mut bytes = Cursor::new(Vec::new());
    image
        .write_to(&mut bytes, ImageFormat::Png)
        .map_err(|err| PipelineError::Extraction(format!("Could not encode page PNG: {err}")))?;
    Ok(general_purpose::STANDARD.encode(bytes.into_inner()))
}

fn render_pdf_pages_png_base64(path: &Path, dpi: u16) -> Result<Vec<String>, PipelineError> {
    let pdfium = lock_pdfium()?;
    let document = pdfium
        .load_pdf_from_file(path, None)
        .map_err(|err| PipelineError::Extraction(format!("Could not open rendered PDF: {err}")))?;
    document
        .pages()
        .iter()
        .map(|page| render_page_png_base64(&page, dpi))
        .collect()
}

fn lock_pdfium() -> Result<MutexGuard<'static, LockedPdfium>, PipelineError> {
    let pdfium = PDFIUM.get_or_init(|| {
        bind_pdfium()
            .map(|pdfium| Mutex::new(LockedPdfium(pdfium)))
            .map_err(|err| format!("PDFium unavailable: {err}"))
    });
    let pdfium = pdfium
        .as_ref()
        .map_err(|err| PipelineError::Extraction(err.clone()))?;
    pdfium
        .lock()
        .map_err(|_| PipelineError::Extraction("PDFium global lock was poisoned".to_string()))
}

fn bind_pdfium() -> Result<Pdfium, Box<dyn std::error::Error + Send + Sync>> {
    if let Some(path) = PDFIUM_LIBRARY_PATH.get() {
        return Pdfium::bind_to_library(path)
            .map(Pdfium::new)
            .map_err(|err| err.into());
    }
    Ok(pdfium_auto::bind_pdfium_silent()?)
}

pub fn extract_pptx_document(
    document_id: String,
    filename: String,
    path: &Path,
    config: &PipelineConfig,
) -> Result<DocumentOutput, PipelineError> {
    let progress = noop_extraction_progress();
    extract_pptx_document_with_progress(document_id, filename, path, config, progress.as_ref())
}

fn extract_pptx_document_with_progress(
    document_id: String,
    filename: String,
    path: &Path,
    config: &PipelineConfig,
    progress: &(dyn Fn(ExtractionProgress) + Send + Sync),
) -> Result<DocumentOutput, PipelineError> {
    let file = File::open(path)
        .map_err(|err| PipelineError::Extraction(format!("Could not open PPTX: {err}")))?;
    let mut archive = SizeLimitedZipArchive::new(file, "PPTX")?;
    let slide_paths = pptx_slide_paths(&mut archive)?;
    let total_pages = slide_paths.len();
    let mut pages = Vec::with_capacity(total_pages);

    let skip_notes = config.skip_tables || config.text_only;
    let skip_slide_tables = config.skip_pptx_tables || config.text_only;
    let rendered_slide_images = if should_render_pptx_slide_screenshots(config) {
        Some(render_pptx_slide_screenshots_base64(
            path,
            total_pages,
            config.pdf_image_dpi.as_u16(),
        )?)
    } else {
        None
    };

    for (index, slide_path) in slide_paths.iter().enumerate() {
        let page_number = index + 1;
        emit_extraction_progress(
            progress,
            page_number,
            total_pages,
            index,
            format!("Extracting slide {page_number} of {total_pages}."),
        );
        let slide_xml = read_zip_string(&mut archive, slide_path)?;
        let (text_parts, tables) = extract_pptx_slide_xml(&slide_xml, skip_slide_tables)?;
        let mut text_parts = text_parts;
        let image_base64 = if config.skip_images || config.text_only {
            None
        } else if let Some(images) = rendered_slide_images.as_ref() {
            images.get(index).cloned()
        } else {
            extract_pptx_slide_image_base64(&mut archive, slide_path, &slide_xml)?
        };

        if !skip_notes {
            if let Some(notes_path) = pptx_notes_path(&mut archive, slide_path)? {
                if let Ok(notes_xml) = read_zip_string(&mut archive, &notes_path) {
                    let notes = extract_pptx_notes_xml(&notes_xml)?;
                    if !notes.is_empty() {
                        text_parts.push(String::new());
                        text_parts.push("[Speaker Notes]".to_string());
                        text_parts.push(notes.join("\n"));
                    }
                }
            }
        }

        pages.push(PageOutput {
            chunk_id: format!("chunk_{page_number}"),
            doc_title: filename.clone(),
            page_number: Some(page_number),
            text: text_parts.join("\n"),
            tables,
            extraction_warnings: Vec::new(),
            html: None,
            embedded_images: Vec::new(),
            image_base64,
            image_text: None,
            image_classifier: None,
            image_text_1: None,
            image_text_2: None,
            image_text_3: None,
            summary_notes: None,
            summary_topics: None,
            summary_relevancy: None,
            summary_quality_validated: None,
            summary_notes_1: None,
            summary_notes_2: None,
            summary_notes_3: None,
            summary_budget_exhausted: None,
        });
        emit_extraction_progress(
            progress,
            page_number,
            total_pages,
            index + 1,
            format!("Extracted slide {page_number} of {total_pages}."),
        );
    }

    Ok(DocumentOutput {
        document: DocumentMetadata {
            document_id,
            filename,
            total_pages,
            metadata: [
                ("source_type".to_string(), serde_json::json!("pptx")),
                ("file_type".to_string(), serde_json::json!(".pptx")),
            ]
            .into_iter()
            .collect(),
        },
        pages,
        metrics: None,
    })
}

fn pptx_slide_paths(archive: &mut SizeLimitedZipArchive) -> Result<Vec<String>, PipelineError> {
    let presentation_xml = read_zip_string(archive, "ppt/presentation.xml")?;
    let rels_xml = read_zip_string(archive, "ppt/_rels/presentation.xml.rels")?;
    let rels = opc::parse_relationships(&rels_xml, "PPTX")?;
    let document = roxmltree::Document::parse(&presentation_xml)
        .map_err(|err| PipelineError::Extraction(format!("Invalid presentation XML: {err}")))?;
    let mut slide_paths = Vec::new();

    for slide in document
        .descendants()
        .filter(|node| node.is_element() && node.tag_name().name() == "sldId")
    {
        let Some(rel_id) = slide
            .attribute((
                "http://schemas.openxmlformats.org/officeDocument/2006/relationships",
                "id",
            ))
            .or_else(|| slide.attribute("id"))
        else {
            continue;
        };
        if let Some(relationship) = rels.get(rel_id) {
            slide_paths.push(opc::normalize_package_path("ppt", &relationship.target));
        }
    }

    if slide_paths.is_empty() {
        slide_paths = (0..archive.len())
            .filter_map(|index| archive.entry_name(index))
            .filter(|name| {
                name.starts_with("ppt/slides/slide")
                    && name.ends_with(".xml")
                    && !name.contains("/_rels/")
            })
            .collect();
        slide_paths.sort_by_key(|path| path_numeric_suffix(path));
    }

    Ok(slide_paths)
}

fn pptx_notes_path(
    archive: &mut SizeLimitedZipArchive,
    slide_path: &str,
) -> Result<Option<String>, PipelineError> {
    let Some((dir, name)) = slide_path.rsplit_once('/') else {
        return Ok(None);
    };
    let rels_path = format!("{dir}/_rels/{name}.rels");
    let Ok(rels_xml) = read_zip_string(archive, &rels_path) else {
        return Ok(None);
    };
    let rels = opc::parse_relationships(&rels_xml, "PPTX")?;
    Ok(rels
        .into_values()
        .find(|relationship| relationship.target.contains("notesSlides/"))
        .map(|relationship| opc::normalize_package_path(dir, &relationship.target)))
}

fn extract_pptx_slide_image_base64(
    archive: &mut SizeLimitedZipArchive,
    slide_path: &str,
    slide_xml: &str,
) -> Result<Option<String>, PipelineError> {
    let Some((dir, name)) = slide_path.rsplit_once('/') else {
        return Ok(None);
    };
    let rels_path = format!("{dir}/_rels/{name}.rels");
    let Ok(rels_xml) = read_zip_string(archive, &rels_path) else {
        return Ok(None);
    };
    let image_targets = opc::parse_relationships(&rels_xml, "PPTX")?;
    if image_targets.is_empty() {
        return Ok(None);
    }

    let document = roxmltree::Document::parse(slide_xml)
        .map_err(|err| PipelineError::Extraction(format!("Invalid slide XML: {err}")))?;
    let mut candidates = Vec::new();
    for blip in document
        .descendants()
        .filter(|node| node.is_element() && node.tag_name().name() == "blip")
    {
        let Some(rel_id) = blip
            .attribute((
                "http://schemas.openxmlformats.org/officeDocument/2006/relationships",
                "embed",
            ))
            .or_else(|| blip.attribute("embed"))
        else {
            continue;
        };
        let Some(relationship) = image_targets
            .get(rel_id)
            .filter(|relationship| relationship.type_ends_with("/image"))
            .filter(|relationship| !relationship.is_external())
        else {
            continue;
        };
        let image_path = opc::normalize_package_path(dir, &relationship.target);
        if let Ok(bytes) = read_zip_bytes(archive, &image_path) {
            candidates.push(bytes);
        }
    }

    let Some(largest_image) = candidates.into_iter().max_by_key(Vec::len) else {
        return Ok(None);
    };
    Ok(Some(opc::image_bytes_to_png_base64(
        &largest_image,
        "PPTX image",
    )?))
}

fn should_render_pptx_slide_screenshots(config: &PipelineConfig) -> bool {
    !config.skip_images
        && !config.text_only
        && !config.extract_only
        && config.vision_mode != VisionMode::None
}

fn render_pptx_slide_screenshots_base64(
    path: &Path,
    expected_slides: usize,
    dpi: u16,
) -> Result<Vec<String>, PipelineError> {
    let soffice = soffice_path().ok_or_else(|| {
        PipelineError::Extraction(
            "Could not render PPTX slide screenshots for vision: LibreOffice/soffice was not found. Install LibreOffice or enable Skip Slide Screenshots.".to_string(),
        )
    })?;
    let temp_dir = tempfile::tempdir().map_err(|err| {
        PipelineError::Extraction(format!("Could not create PPTX render workspace: {err}"))
    })?;
    let output_dir = temp_dir.path().join("out");
    let profile_dir = temp_dir.path().join("profile");
    fs::create_dir_all(&output_dir).map_err(|err| {
        PipelineError::Extraction(format!(
            "Could not create PPTX render output directory: {err}"
        ))
    })?;
    fs::create_dir_all(&profile_dir).map_err(|err| {
        PipelineError::Extraction(format!(
            "Could not create LibreOffice profile directory: {err}"
        ))
    })?;
    let profile_uri = libreoffice_profile_uri(&profile_dir)?;

    let mut command = Command::new(&soffice);
    suppress_command_window(&mut command);
    let output = command
        .arg(format!("-env:UserInstallation={profile_uri}"))
        .arg("--headless")
        .arg("--convert-to")
        .arg(pptx_pdf_export_filter())
        .arg("--outdir")
        .arg(&output_dir)
        .arg(path)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .map_err(|err| {
            PipelineError::Extraction(format!(
                "Could not start LibreOffice for PPTX render: {err}"
            ))
        })?;

    if !output.status.success() {
        return Err(PipelineError::Extraction(format!(
            "LibreOffice failed to render PPTX slides: status={} stdout={} stderr={}",
            output.status,
            String::from_utf8_lossy(&output.stdout).trim(),
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }

    let pdf_path = converted_pdf_path(path, &output_dir).ok_or_else(|| {
        PipelineError::Extraction(format!(
            "LibreOffice did not produce a PDF for PPTX render. stdout={} stderr={}",
            String::from_utf8_lossy(&output.stdout).trim(),
            String::from_utf8_lossy(&output.stderr).trim()
        ))
    })?;
    let rendered_pages = render_pdf_pages_png_base64(&pdf_path, dpi)?;
    if rendered_pages.len() != expected_slides {
        return Err(PipelineError::Extraction(format!(
            "LibreOffice rendered {} PPTX slide images, expected {expected_slides}",
            rendered_pages.len()
        )));
    }
    Ok(rendered_pages)
}

fn pptx_pdf_export_filter() -> &'static str {
    r#"pdf:impress_pdf_Export:{"ExportHiddenSlides":{"type":"boolean","value":"true"}}"#
}

fn soffice_path() -> Option<PathBuf> {
    resolve_soffice()
}

fn libreoffice_profile_uri(profile_dir: &Path) -> Result<String, PipelineError> {
    Url::from_directory_path(profile_dir)
        .map(|url| url.to_string())
        .map_err(|_| {
            PipelineError::Extraction(format!(
                "Could not convert LibreOffice profile directory to a file URI: {}",
                profile_dir.display()
            ))
        })
}

fn converted_pdf_path(input_path: &Path, output_dir: &Path) -> Option<PathBuf> {
    let expected = input_path
        .file_stem()
        .and_then(|stem| stem.to_str())
        .map(|stem| output_dir.join(format!("{stem}.pdf")));
    if let Some(path) = expected {
        if path.exists() {
            return Some(path);
        }
    }

    fs::read_dir(output_dir)
        .ok()?
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .find(|path| {
            path.extension()
                .and_then(|extension| extension.to_str())
                .is_some_and(|extension| extension.eq_ignore_ascii_case("pdf"))
        })
}

fn extract_pptx_slide_xml(
    xml: &str,
    skip_tables: bool,
) -> Result<(Vec<String>, SlideTables), PipelineError> {
    let document = roxmltree::Document::parse(xml)
        .map_err(|err| PipelineError::Extraction(format!("Invalid slide XML: {err}")))?;
    let text_parts = extract_pptx_text_paragraphs(&document, true);
    let tables = if skip_tables {
        Vec::new()
    } else {
        document
            .descendants()
            .filter(|node| node.is_element() && node.tag_name().name() == "tbl")
            .filter_map(extract_pptx_table)
            .collect()
    };

    Ok((text_parts, tables))
}

fn extract_pptx_notes_xml(xml: &str) -> Result<Vec<String>, PipelineError> {
    let document = roxmltree::Document::parse(xml)
        .map_err(|err| PipelineError::Extraction(format!("Invalid notes XML: {err}")))?;
    Ok(extract_pptx_text_paragraphs(&document, false))
}

fn extract_pptx_text_paragraphs(
    document: &roxmltree::Document<'_>,
    skip_table_ancestors: bool,
) -> Vec<String> {
    document
        .descendants()
        .filter(|node| node.is_element() && node.tag_name().name() == "p")
        .filter(|node| !skip_table_ancestors || !has_ancestor_named(*node, "tbl"))
        .filter_map(|paragraph| {
            let text = paragraph
                .descendants()
                .filter(|node| node.is_element() && node.tag_name().name() == "t")
                .filter_map(|node| node.text())
                .collect::<String>();
            clean_xml_text(Some(&text))
        })
        .collect()
}

fn extract_pptx_table(table: roxmltree::Node<'_, '_>) -> Option<Vec<Vec<String>>> {
    let rows: Vec<Vec<String>> = table
        .children()
        .filter(|node| node.is_element() && node.tag_name().name() == "tr")
        .filter_map(|row| {
            let cells: Vec<String> = row
                .children()
                .filter(|node| node.is_element() && node.tag_name().name() == "tc")
                .filter(|cell| !is_merge_continuation(*cell))
                .map(|cell| {
                    cell.descendants()
                        .filter(|node| node.is_element() && node.tag_name().name() == "t")
                        .filter_map(|node| clean_xml_text(node.text()))
                        .collect::<Vec<_>>()
                        .join("\n")
                })
                .collect();

            if cells.is_empty() {
                None
            } else {
                Some(cells)
            }
        })
        .collect();

    if rows.is_empty() {
        None
    } else {
        Some(rows)
    }
}

fn is_merge_continuation(cell: roxmltree::Node<'_, '_>) -> bool {
    cell.children()
        .find(|node| node.is_element() && node.tag_name().name() == "tcPr")
        .is_some_and(|properties| {
            properties.attribute("hMerge") == Some("1")
                || properties.attribute("vMerge") == Some("1")
        })
}

fn has_ancestor_named(node: roxmltree::Node<'_, '_>, name: &str) -> bool {
    node.ancestors()
        .skip(1)
        .any(|ancestor| ancestor.is_element() && ancestor.tag_name().name() == name)
}

fn clean_xml_text(text: Option<&str>) -> Option<String> {
    text.map(str::trim)
        .filter(|text| !text.is_empty())
        .map(ToString::to_string)
}

fn read_zip_string(
    archive: &mut SizeLimitedZipArchive,
    path: &str,
) -> Result<String, PipelineError> {
    archive.read_string(path, "PPTX")
}

fn read_zip_bytes(
    archive: &mut SizeLimitedZipArchive,
    path: &str,
) -> Result<Vec<u8>, PipelineError> {
    archive.read_bytes(path, "PPTX")
}

fn path_numeric_suffix(path: &str) -> usize {
    path.rsplit_once('/')
        .map(|(_, name)| name)
        .unwrap_or(path)
        .trim_start_matches("slide")
        .trim_end_matches(".xml")
        .parse()
        .unwrap_or(usize::MAX)
}

impl Default for Extractor {
    fn default() -> Self {
        Self::new()
    }
}

pub fn extract_text_document(
    document_id: String,
    filename: String,
    text: &str,
    config: &PipelineConfig,
) -> DocumentOutput {
    let progress = noop_extraction_progress();
    extract_text_document_with_progress(document_id, filename, text, config, progress.as_ref())
}

fn extract_text_document_with_progress(
    document_id: String,
    filename: String,
    text: &str,
    config: &PipelineConfig,
    progress: &(dyn Fn(ExtractionProgress) + Send + Sync),
) -> DocumentOutput {
    let chunks = split_recursive(text, config.chunk_size, config.chunk_overlap);
    let chunks = if chunks.is_empty() {
        vec![String::new()]
    } else {
        chunks
    };

    let total_pages = chunks.len();
    let mut pages = Vec::with_capacity(total_pages);
    for (index, text) in chunks.into_iter().enumerate() {
        let page_number = index + 1;
        emit_extraction_progress(
            progress,
            page_number,
            total_pages,
            index,
            format!("Preparing chunk {page_number} of {total_pages}."),
        );
        pages.push(PageOutput {
            chunk_id: format!("chunk_{}", index + 1),
            doc_title: filename.clone(),
            page_number: Some(index + 1),
            text,
            tables: Vec::new(),
            extraction_warnings: Vec::new(),
            html: None,
            embedded_images: Vec::new(),
            image_base64: None,
            image_text: None,
            image_classifier: None,
            image_text_1: None,
            image_text_2: None,
            image_text_3: None,
            summary_notes: None,
            summary_topics: None,
            summary_relevancy: None,
            summary_quality_validated: None,
            summary_notes_1: None,
            summary_notes_2: None,
            summary_notes_3: None,
            summary_budget_exhausted: None,
        });
        emit_extraction_progress(
            progress,
            page_number,
            total_pages,
            index + 1,
            format!("Prepared chunk {page_number} of {total_pages}."),
        );
    }

    DocumentOutput {
        document: DocumentMetadata {
            document_id,
            filename,
            total_pages,
            metadata: Default::default(),
        },
        pages,
        metrics: None,
    }
}

fn noop_extraction_progress() -> ExtractionProgressCallback {
    Arc::new(|_| {})
}

fn emit_extraction_progress(
    progress: &(dyn Fn(ExtractionProgress) + Send + Sync),
    page_number: usize,
    total_pages: usize,
    completed_pages: usize,
    message: impl Into<String>,
) {
    progress(ExtractionProgress {
        page_number,
        total_pages,
        completed_pages,
        message: message.into(),
    });
}

pub fn split_recursive(text: &str, chunk_size: usize, chunk_overlap: usize) -> Vec<String> {
    if text.is_empty() {
        return Vec::new();
    }

    if text.chars().count() <= chunk_size {
        return vec![text.to_string()];
    }

    let overlap = chunk_overlap.min(chunk_size.saturating_sub(1));
    let separators = ["\n\n", "\n", ". ", " ", ""];
    let mut chunks = Vec::new();
    let mut start = 0;
    let char_indices: Vec<usize> = text.char_indices().map(|(index, _)| index).collect();

    while start < text.len() {
        let previous_start = start;
        let start_char = char_indices.partition_point(|&idx| idx < start);
        let end_char = (start_char + chunk_size).min(char_indices.len());
        let hard_end = if end_char >= char_indices.len() {
            text.len()
        } else {
            char_indices[end_char]
        };

        let window = &text[start..hard_end];
        let split_at = best_split(window, &separators).unwrap_or(window.len());
        let end = if split_at == 0 {
            hard_end
        } else {
            start + split_at
        };
        let chunk = text[start..end].trim().to_string();
        if !chunk.is_empty() {
            chunks.push(chunk);
        }

        if end >= text.len() {
            break;
        }

        let next_start_char = char_indices.partition_point(|&idx| idx < end);
        let rewind_char = next_start_char.saturating_sub(overlap);
        start = char_indices.get(rewind_char).copied().unwrap_or(end);
        if start <= previous_start || start >= end {
            start = end;
        }
    }

    chunks
}

fn best_split(window: &str, separators: &[&str]) -> Option<usize> {
    for separator in separators {
        if separator.is_empty() {
            return Some(window.len());
        }
        if let Some(index) = window.rfind(separator) {
            let split = index + separator.len();
            if split > separator.len() {
                return Some(split);
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::{libreoffice_profile_uri, SizeLimitedZipArchive, MAX_DECOMPRESSED_ARCHIVE_BYTES};
    use std::{
        fs::File,
        io::{Cursor, Write},
    };
    use zip::{write::SimpleFileOptions, CompressionMethod, ZipWriter};

    #[test]
    fn archive_size_limit_rejects_declared_entry_over_limit() {
        let temp = tempfile::NamedTempFile::new().unwrap();
        let file = File::create(temp.path()).unwrap();
        ZipWriter::new(file).finish().unwrap();
        let file = File::open(temp.path()).unwrap();
        let archive = SizeLimitedZipArchive::new(file, "test").unwrap();

        let error = archive
            .ensure_declared_size_can_be_read(
                MAX_DECOMPRESSED_ARCHIVE_BYTES + 1,
                "test",
                "huge.bin",
            )
            .unwrap_err();

        assert!(error
            .to_string()
            .contains("exceeds decompressed size limit"));
    }

    #[test]
    fn archive_read_rejects_inflated_data_beyond_declared_size() {
        let temp = tempfile::NamedTempFile::new().unwrap();
        write_zip_with_declared_uncompressed_size(temp.path(), "word/document.xml", 1, 4096);
        let file = File::open(temp.path()).unwrap();
        let mut archive = SizeLimitedZipArchive::new(file, "DOCX").unwrap();

        let error = archive.read_bytes("word/document.xml", "DOCX").unwrap_err();

        assert!(error.to_string().contains("inflated beyond declared size"));
    }

    #[test]
    fn archive_repeated_reads_count_entry_once() {
        let temp = tempfile::NamedTempFile::new().unwrap();
        let file = File::create(temp.path()).unwrap();
        let mut writer = ZipWriter::new(file);
        let options = SimpleFileOptions::default().compression_method(CompressionMethod::Stored);
        writer.start_file("word/document.xml", options).unwrap();
        writer.write_all(b"hello").unwrap();
        writer.finish().unwrap();
        let file = File::open(temp.path()).unwrap();
        let mut archive = SizeLimitedZipArchive::new(file, "DOCX").unwrap();

        assert_eq!(
            archive.read_string("word/document.xml", "DOCX").unwrap(),
            "hello"
        );
        assert_eq!(
            archive.read_string("word/document.xml", "DOCX").unwrap(),
            "hello"
        );
        assert_eq!(archive.decompressed_bytes, 5);
    }

    #[test]
    fn libreoffice_profile_uri_uses_file_url_format() {
        let temp = tempfile::tempdir().unwrap();
        let profile_dir = temp.path().join("profile dir");
        std::fs::create_dir(&profile_dir).unwrap();

        let uri = libreoffice_profile_uri(&profile_dir).unwrap();

        assert!(uri.starts_with("file://"));
        assert!(uri.ends_with('/'));
        assert!(uri.contains("profile%20dir"));
    }

    fn write_zip_with_declared_uncompressed_size(
        path: &std::path::Path,
        entry_name: &str,
        declared_size: u32,
        actual_size: usize,
    ) {
        let mut buffer = Cursor::new(Vec::new());
        {
            let mut writer = ZipWriter::new(&mut buffer);
            let options =
                SimpleFileOptions::default().compression_method(CompressionMethod::Deflated);
            writer.start_file(entry_name, options).unwrap();
            writer.write_all(&vec![b'a'; actual_size]).unwrap();
            writer.finish().unwrap();
        }
        let mut bytes = buffer.into_inner();
        patch_zip_uncompressed_sizes(&mut bytes, declared_size);
        std::fs::write(path, bytes).unwrap();
    }

    fn patch_zip_uncompressed_sizes(bytes: &mut [u8], declared_size: u32) {
        let mut index = 0;
        while index + 30 <= bytes.len() {
            let signature = u32::from_le_bytes(bytes[index..index + 4].try_into().unwrap());
            match signature {
                0x0403_4b50 => {
                    bytes[index + 22..index + 26].copy_from_slice(&declared_size.to_le_bytes());
                    index += 30;
                }
                0x0201_4b50 => {
                    bytes[index + 24..index + 28].copy_from_slice(&declared_size.to_le_bytes());
                    index += 46;
                }
                _ => index += 1,
            }
        }
    }
}
