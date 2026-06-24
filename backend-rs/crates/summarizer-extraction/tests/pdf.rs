use summarizer_extraction::Extractor;
use summarizer_types::PipelineConfig;

#[tokio::test]
async fn pdf_extraction_reads_text_and_renders_images_when_enabled() {
    if std::env::var("RUN_PDFIUM_TESTS").ok().as_deref() != Some("1") {
        eprintln!("SKIPPED: set RUN_PDFIUM_TESTS=1 to run PDF extraction coverage");
        return;
    }

    let pdf = fixture_pdf();

    let output = Extractor::new()
        .extract_path(
            "doc_pdf".to_string(),
            &pdf,
            &PipelineConfig {
                run_summarization: false,
                ..PipelineConfig::default()
            },
        )
        .await
        .unwrap();

    assert_eq!(output.document.total_pages, 7);
    assert_eq!(output.pages.len(), 7);
    assert!(output.pages.iter().any(|page| page.text.contains("AI")));
    assert!(output.pages.iter().all(|page| page.image_base64.is_some()));
}

#[tokio::test]
async fn pdf_extraction_respects_skip_images() {
    if std::env::var("RUN_PDFIUM_TESTS").ok().as_deref() != Some("1") {
        eprintln!("SKIPPED: set RUN_PDFIUM_TESTS=1 to run PDF extraction coverage");
        return;
    }

    let pdf = fixture_pdf();

    let output = Extractor::new()
        .extract_path(
            "doc_pdf".to_string(),
            &pdf,
            &PipelineConfig {
                skip_images: true,
                run_summarization: false,
                ..PipelineConfig::default()
            },
        )
        .await
        .unwrap();

    assert!(output.pages.iter().all(|page| page.image_base64.is_none()));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn concurrent_pdf_extractions_complete() {
    if std::env::var("RUN_PDFIUM_TESTS").ok().as_deref() != Some("1") {
        eprintln!("SKIPPED: set RUN_PDFIUM_TESTS=1 to run PDF extraction coverage");
        return;
    }

    let pdf = fixture_pdf();
    let config = PipelineConfig {
        skip_images: true,
        run_summarization: false,
        ..PipelineConfig::default()
    };

    let first_extractor = Extractor::new();
    let second_extractor = Extractor::new();
    let first = first_extractor.extract_path("doc_pdf_1".to_string(), &pdf, &config);
    let second = second_extractor.extract_path("doc_pdf_2".to_string(), &pdf, &config);
    let (first, second) = tokio::join!(first, second);

    let first = first.unwrap();
    let second = second.unwrap();
    assert_eq!(first.document.total_pages, 7);
    assert_eq!(second.document.total_pages, 7);
    assert!(first.pages.iter().any(|page| page.text.contains("AI")));
    assert!(second.pages.iter().any(|page| page.text.contains("AI")));
}

fn fixture_pdf() -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(3)
        .unwrap()
        .join("input")
        .join("ai-transparency-contact-center-summarization-technical-note.pdf")
}
