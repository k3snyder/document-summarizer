#![allow(
    clippy::await_holding_lock,
    reason = "tests serialize process-wide environment variable overrides"
)]

use std::{
    io::{Cursor, Write},
    sync::{Arc, Mutex},
};
use summarizer_pipeline::{Pipeline, PipelineProgressStage};
use summarizer_types::{
    CliProvider, PipelineConfig, SummarizerMode, SummarizerProvider, VisionMode,
};
use wiremock::matchers::{body_string_contains, method, path as request_path};
use wiremock::{Mock, MockServer, ResponseTemplate};

static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

#[tokio::test]
async fn pipeline_processes_text_file() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("fixture.txt");
    tokio::fs::write(&path, "First line\nSecond line")
        .await
        .unwrap();

    let output = Pipeline::new()
        .run_path(
            "job_1",
            &path,
            &PipelineConfig {
                run_summarization: false,
                ..PipelineConfig::default()
            },
        )
        .await
        .unwrap();

    assert_eq!(output.document.filename, "fixture.txt");
    assert_eq!(output.pages.len(), 1);
    assert_eq!(output.pages[0].summary_relevancy, None);
}

#[tokio::test]
async fn pipeline_emits_stage_progress() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("fixture.txt");
    tokio::fs::write(&path, "First line\nSecond line")
        .await
        .unwrap();

    let events = Arc::new(Mutex::new(Vec::new()));
    let captured_events = Arc::clone(&events);

    Pipeline::new()
        .run_path_with_progress(
            "job_progress",
            &path,
            &PipelineConfig {
                summarizer_mode: SummarizerMode::Skip,
                ..PipelineConfig::default()
            },
            move |progress| {
                captured_events.lock().unwrap().push(progress);
            },
        )
        .await
        .unwrap();

    let events = events.lock().unwrap();
    assert!(events
        .iter()
        .any(|event| event.stage == PipelineProgressStage::Extraction));
    assert_eq!(events.last().unwrap().progress, 99);
    assert_eq!(events.last().unwrap().total_pages, Some(1));
}

#[tokio::test(flavor = "current_thread")]
async fn pipeline_runs_openai_compatible_summarization_and_records_metrics() {
    let _env_guard = ENV_LOCK.lock().unwrap();
    let server = MockServer::start().await;
    mock_chat(&server, "accurately summarizing", "* First note", 13).await;
    mock_chat(&server, "Compare the following original text", "92%", 7).await;
    mock_chat(&server, "Analyze the topic categories", "Operations", 5).await;

    std::env::set_var("OPENAI_BASE_URL", server.uri());
    std::env::set_var("OPENAI_MODEL", "summary-model");

    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("fixture.txt");
    tokio::fs::write(&path, "First line\nSecond line")
        .await
        .unwrap();

    let output = Pipeline::new()
        .run_path(
            "job_summary",
            &path,
            &PipelineConfig {
                summarizer_provider: SummarizerProvider::Openai,
                ..PipelineConfig::default()
            },
        )
        .await
        .unwrap();

    std::env::remove_var("OPENAI_BASE_URL");
    std::env::remove_var("OPENAI_MODEL");

    assert_eq!(
        output.pages[0].summary_notes,
        Some(vec!["First note".to_string()])
    );
    assert_eq!(output.pages[0].summary_relevancy, Some(92));
    let metrics = output.metrics.unwrap();
    assert_eq!(
        metrics.config.summarizer_provider.as_deref(),
        Some("openai")
    );
    assert_eq!(metrics.stages.summarization.tokens, 25);
    assert!(metrics.stages.extraction.tokens > 0);
    assert_eq!(metrics.stages.summarization.total_attempts, Some(1));
    assert_eq!(metrics.stages.summarization.avg_relevancy, Some(92));
    assert!(metrics.total_tokens > metrics.stages.summarization.tokens);
}

#[tokio::test]
async fn pipeline_records_empty_vision_stage_when_no_pages_have_images() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("fixture.txt");
    tokio::fs::write(&path, "Text-only content").await.unwrap();

    let output = Pipeline::new()
        .run_path(
            "job_no_images",
            &path,
            &PipelineConfig {
                vision_mode: VisionMode::Openai,
                run_summarization: false,
                ..PipelineConfig::default()
            },
        )
        .await
        .unwrap();

    let metrics = output.metrics.unwrap();
    assert_eq!(metrics.stages.vision.pages_with_images, Some(0));
    assert_eq!(metrics.stages.vision.extracted_count, None);
}

#[tokio::test]
async fn pipeline_metrics_record_resolved_vision_providers() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("fixture.txt");
    tokio::fs::write(&path, "Text-only content").await.unwrap();

    let output = Pipeline::new()
        .run_path(
            "job_resolved_providers",
            &path,
            &PipelineConfig {
                vision_mode: VisionMode::Claude,
                vision_cli_provider: Some(CliProvider::Codex),
                summarizer_mode: SummarizerMode::Skip,
                ..PipelineConfig::default()
            },
        )
        .await
        .unwrap();

    let metrics = output.metrics.unwrap();
    assert_eq!(
        metrics.config.vision_extractor_provider.as_deref(),
        Some("codex")
    );
    assert_eq!(
        metrics.config.vision_classifier_provider.as_deref(),
        Some("codex")
    );
    assert_eq!(metrics.config.summarizer_provider, None);
}

#[tokio::test(flavor = "current_thread")]
async fn pipeline_runs_docx_embedded_image_vision_when_classification_is_skipped() {
    let _env_guard = ENV_LOCK.lock().unwrap();
    let server = MockServer::start().await;
    mock_chat_many(
        &server,
        "Analyze this document page image",
        "vision found the embedded diagram",
        11,
    )
    .await;

    std::env::set_var("OPENAI_BASE_URL", server.uri());
    std::env::set_var("OPENAI_VISION_MODEL", "vision-model");

    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("fixture.docx");
    write_docx_with_embedded_image(&path);

    let output = Pipeline::new()
        .run_path(
            "job_docx_vision",
            &path,
            &PipelineConfig {
                vision_mode: VisionMode::Openai,
                vision_skip_classification: true,
                run_summarization: false,
                ..PipelineConfig::default()
            },
        )
        .await
        .unwrap();

    std::env::remove_var("OPENAI_BASE_URL");
    std::env::remove_var("OPENAI_VISION_MODEL");

    assert_eq!(
        output.document.metadata["source_type"],
        serde_json::json!("docx")
    );
    assert_eq!(
        output.pages[0].image_text.as_deref(),
        Some("vision found the embedded diagram")
    );
    assert_eq!(output.pages[0].image_base64, None);
    assert!(output.pages[0]
        .embedded_images
        .iter()
        .all(|image| image.base64.is_none()));
    let metrics = output.metrics.unwrap();
    assert_eq!(metrics.stages.vision.pages_with_images, Some(1));
    assert_eq!(metrics.stages.vision.classified_count, Some(0));
    assert_eq!(metrics.stages.vision.extracted_count, Some(1));
    assert_eq!(
        metrics.config.vision_extractor_provider.as_deref(),
        Some("openai")
    );
    assert_eq!(metrics.config.vision_classifier_provider, None);
}

#[tokio::test]
async fn pipeline_processes_pdf_when_pdfium_is_enabled() {
    if std::env::var("RUN_PDFIUM_TESTS").ok().as_deref() != Some("1") {
        eprintln!("SKIPPED: set RUN_PDFIUM_TESTS=1 to run PDF pipeline coverage");
        return;
    }

    let repo = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(3)
        .unwrap()
        .to_path_buf();
    let path = repo
        .join("input")
        .join("ai-transparency-contact-center-summarization-technical-note.pdf");

    let output = Pipeline::new()
        .run_path(
            "job_pdf",
            &path,
            &PipelineConfig {
                skip_images: true,
                run_summarization: false,
                ..PipelineConfig::default()
            },
        )
        .await
        .unwrap();

    assert_eq!(output.document.total_pages, 7);
    assert!(output.pages.iter().any(|page| page.text.contains("AI")));
    assert!(output.metrics.is_some());
}

#[tokio::test]
async fn pipeline_processes_pptx_content() {
    let repo = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(3)
        .unwrap()
        .to_path_buf();
    let path = repo
        .join("backend-rs")
        .join("tests")
        .join("fixtures")
        .join("golden")
        .join("test_presentation.pptx");

    let output = Pipeline::new()
        .run_path(
            "job_pptx",
            &path,
            &PipelineConfig {
                skip_images: true,
                run_summarization: false,
                ..PipelineConfig::default()
            },
        )
        .await
        .unwrap();

    assert_eq!(output.document.total_pages, 6);
    assert!(output.pages[1].text.contains("Remember to explain this"));
    assert_eq!(output.pages[2].tables[0][0], ["Header1", "Header2"]);
    assert!(output.metrics.is_some());
}

#[tokio::test(flavor = "current_thread")]
async fn pipeline_runs_pptx_vision_on_every_slide_when_classification_is_skipped() {
    if !soffice_available() {
        return;
    }

    let _env_guard = ENV_LOCK.lock().unwrap();
    let server = MockServer::start().await;
    mock_chat_many(
        &server,
        "Analyze this document page image",
        "vision extracted slide",
        3,
    )
    .await;

    std::env::set_var("OPENAI_BASE_URL", server.uri());
    std::env::set_var("OPENAI_VISION_MODEL", "vision-model");

    let repo = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(3)
        .unwrap()
        .to_path_buf();
    let path = repo
        .join("backend-rs")
        .join("tests")
        .join("fixtures")
        .join("golden")
        .join("test_presentation.pptx");

    let output = Pipeline::new()
        .run_path(
            "job_pptx_vision",
            &path,
            &PipelineConfig {
                vision_mode: VisionMode::Openai,
                vision_skip_classification: true,
                run_summarization: false,
                ..PipelineConfig::default()
            },
        )
        .await
        .unwrap();

    std::env::remove_var("OPENAI_BASE_URL");
    std::env::remove_var("OPENAI_VISION_MODEL");

    assert_eq!(output.pages.len(), 6);
    assert!(output
        .pages
        .iter()
        .all(|page| page.image_text.as_deref() == Some("vision extracted slide")));
    let metrics = output.metrics.unwrap();
    assert_eq!(metrics.stages.vision.pages_with_images, Some(6));
    assert_eq!(metrics.stages.vision.classified_count, Some(0));
    assert_eq!(metrics.stages.vision.extracted_count, Some(6));
}

async fn mock_chat(server: &MockServer, body_contains: &str, content: &str, total_tokens: usize) {
    Mock::given(method("POST"))
        .and(request_path("/chat/completions"))
        .and(body_string_contains(body_contains))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "choices": [{"message": {"content": content}}],
            "usage": {"total_tokens": total_tokens}
        })))
        .expect(1)
        .mount(server)
        .await;
}

async fn mock_chat_many(
    server: &MockServer,
    body_contains: &str,
    content: &str,
    total_tokens: usize,
) {
    Mock::given(method("POST"))
        .and(request_path("/chat/completions"))
        .and(body_string_contains(body_contains))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "choices": [{"message": {"content": content}}],
            "usage": {"total_tokens": total_tokens}
        })))
        .mount(server)
        .await;
}

fn write_docx_with_embedded_image(path: &std::path::Path) {
    let file = std::fs::File::create(path).unwrap();
    let mut archive = zip::ZipWriter::new(file);
    let options =
        zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);

    write_zip_entry(
        &mut archive,
        options,
        "[Content_Types].xml",
        br#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types">
  <Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/>
  <Default Extension="xml" ContentType="application/xml"/>
  <Default Extension="png" ContentType="image/png"/>
</Types>"#,
    );
    write_zip_entry(
        &mut archive,
        options,
        "word/_rels/document.xml.rels",
        br#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">
  <Relationship Id="rIdImage" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/image" Target="media/image1.png"/>
</Relationships>"#,
    );
    write_zip_entry(
        &mut archive,
        options,
        "word/document.xml",
        br#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main"
  xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships"
  xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main"
  xmlns:wp="http://schemas.openxmlformats.org/drawingml/2006/wordprocessingDrawing">
  <w:body>
    <w:p><w:r><w:t>Document with embedded image</w:t></w:r></w:p>
    <w:p><w:r><w:drawing><wp:inline><wp:docPr id="1" name="Picture 1" descr="Embedded diagram"/><a:graphic><a:graphicData><a:blip r:embed="rIdImage"/></a:graphicData></a:graphic></wp:inline></w:drawing></w:r></w:p>
  </w:body>
</w:document>"#,
    );
    write_zip_entry(
        &mut archive,
        options,
        "word/media/image1.png",
        &sample_png_bytes(),
    );
    archive.finish().unwrap();
}

fn sample_png_bytes() -> Vec<u8> {
    let image = image::RgbaImage::from_pixel(1, 1, image::Rgba([0, 128, 255, 255]));
    let mut bytes = Cursor::new(Vec::new());
    image::DynamicImage::ImageRgba8(image)
        .write_to(&mut bytes, image::ImageFormat::Png)
        .unwrap();
    bytes.into_inner()
}

fn soffice_available() -> bool {
    std::process::Command::new("sh")
        .arg("-c")
        .arg("command -v soffice >/dev/null 2>&1 || command -v libreoffice >/dev/null 2>&1 || test -x /Applications/LibreOffice.app/Contents/MacOS/soffice || test -x /opt/homebrew/bin/soffice || test -x /usr/local/bin/soffice")
        .status()
        .is_ok_and(|status| status.success())
}

fn write_zip_entry(
    archive: &mut zip::ZipWriter<std::fs::File>,
    options: zip::write::SimpleFileOptions,
    path: &str,
    contents: &[u8],
) {
    archive.start_file(path, options).unwrap();
    archive.write_all(contents).unwrap();
}
