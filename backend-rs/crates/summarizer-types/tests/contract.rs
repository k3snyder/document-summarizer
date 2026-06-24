use serde_json::json;
use summarizer_types::{
    CliProvider, DocumentOutput, PdfImageDpi, PipelineConfig, SummarizerProvider, VisionMode,
};

#[test]
fn pipeline_config_defaults_match_python_backend() {
    let config = PipelineConfig::default();

    assert!(config.run_extraction);
    assert!(!config.extract_only);
    assert_eq!(config.pdf_image_dpi, PdfImageDpi::Dpi200);
    assert_eq!(config.vision_mode, VisionMode::None);
    assert_eq!(config.chunk_size, 3000);
    assert_eq!(config.chunk_overlap, 80);
    assert!(config.run_summarization);
    assert_eq!(config.max_tokens_per_page, 100_000);
    assert_eq!(config.max_seconds_per_page, 300);
}

#[test]
fn pipeline_config_accepts_frontend_numeric_pdf_dpi() {
    let config: PipelineConfig = serde_json::from_value(json!({
        "pdf_image_dpi": 200,
        "vision_mode": "llama_cpp",
        "summarizer_provider": "llama_cpp"
    }))
    .unwrap();

    assert_eq!(config.pdf_image_dpi, PdfImageDpi::Dpi200);
    assert_eq!(config.vision_mode, VisionMode::LlamaCpp);
}

#[test]
fn pipeline_config_accepts_grok_cli_providers() {
    let config: PipelineConfig = serde_json::from_value(json!({
        "vision_mode": "grok",
        "vision_cli_provider": "grok",
        "summarizer_provider": "grok",
        "summarizer_cli_provider": "grok"
    }))
    .unwrap();

    assert_eq!(config.vision_mode, VisionMode::Grok);
    assert_eq!(config.vision_cli_provider, Some(CliProvider::Grok));
    assert_eq!(config.summarizer_provider, SummarizerProvider::Grok);
    assert_eq!(config.summarizer_cli_provider, Some(CliProvider::Grok));
}

#[test]
fn pipeline_config_accepts_deprecated_vision_detailed_extraction_but_does_not_emit_it() {
    let config: PipelineConfig = serde_json::from_value(json!({
        "vision_detailed_extraction": true
    }))
    .unwrap();

    assert!(config.vision_detailed_extraction);
    let value = serde_json::to_value(config).unwrap();
    assert!(value.get("vision_detailed_extraction").is_none());
}

#[test]
fn pipeline_config_round_trips_quality_budget_fields() {
    let config: PipelineConfig = serde_json::from_value(json!({
        "max_tokens_per_page": 5000,
        "max_seconds_per_page": 45
    }))
    .unwrap();

    assert_eq!(config.max_tokens_per_page, 5000);
    assert_eq!(config.max_seconds_per_page, 45);

    let value = serde_json::to_value(config).unwrap();
    assert_eq!(value["max_tokens_per_page"], 5000);
    assert_eq!(value["max_seconds_per_page"], 45);
}

#[test]
fn document_output_accepts_canonical_schema() {
    let output: DocumentOutput = serde_json::from_value(json!({
        "document": {
            "document_id": "doc_fixture",
            "filename": "fixture.pdf",
            "total_pages": 1,
            "metadata": {}
        },
        "pages": [{
            "chunk_id": "chunk_1",
            "doc_title": "fixture.pdf",
            "page_number": 1,
            "text": "hello",
            "tables": [[["A", "B"], ["1", "2"]]],
            "image_base64": null,
            "image_text": null,
            "image_classifier": false,
            "summary_notes": ["note"],
            "summary_topics": ["topic"],
            "summary_relevancy": 92.0,
            "summary_quality_validated": true,
            "summary_budget_exhausted": "tokens"
        }]
    }))
    .unwrap();

    assert_eq!(output.pages[0].tables[0][0], ["A", "B"]);
    assert!(output.pages[0].embedded_images.is_empty());
    assert_eq!(output.pages[0].html, None);
    assert_eq!(output.pages[0].image_base64, None);
    assert_eq!(output.pages[0].summary_relevancy, Some(92));
    assert_eq!(output.pages[0].summary_quality_validated, Some(true));
    assert_eq!(
        output.pages[0].summary_budget_exhausted.as_deref(),
        Some("tokens")
    );
}

#[test]
fn document_output_omits_detailed_fields_when_not_populated() {
    let output: DocumentOutput = serde_json::from_value(json!({
        "document": {
            "document_id": "doc_fixture",
            "filename": "fixture.pdf",
            "total_pages": 1,
            "metadata": {}
        },
        "pages": [{
            "chunk_id": "chunk_1",
            "doc_title": "fixture.pdf",
            "page_number": 1,
            "text": "hello",
            "tables": [],
            "image_base64": null,
            "image_text": null,
            "image_classifier": false,
            "summary_notes": ["note"],
            "summary_topics": ["topic"],
            "summary_relevancy": 92
        }]
    }))
    .unwrap();

    let value = serde_json::to_value(output).unwrap();
    let page = value["pages"][0].as_object().unwrap();
    for key in [
        "image_text_1",
        "image_text_2",
        "image_text_3",
        "summary_notes_1",
        "summary_notes_2",
        "summary_notes_3",
        "summary_quality_validated",
    ] {
        assert!(!page.contains_key(key), "{key} should be omitted");
    }
}

#[test]
fn document_output_includes_detailed_fields_when_populated() {
    let output: DocumentOutput = serde_json::from_value(json!({
        "document": {
            "document_id": "doc_fixture",
            "filename": "fixture.pdf",
            "total_pages": 1,
            "metadata": {}
        },
        "pages": [{
            "chunk_id": "chunk_1",
            "doc_title": "fixture.pdf",
            "page_number": 1,
            "text": "hello",
            "tables": [],
            "image_text_1": "vision 1",
            "image_text_2": "vision 2",
            "image_text_3": "vision 3",
            "summary_notes_1": ["summary 1"],
            "summary_notes_2": ["summary 2"],
            "summary_notes_3": ["summary 3"]
        }]
    }))
    .unwrap();

    let value = serde_json::to_value(output).unwrap();
    let page = &value["pages"][0];
    assert_eq!(page["image_text_1"], "vision 1");
    assert_eq!(page["summary_notes_1"], json!(["summary 1"]));
}

#[test]
fn document_output_serializes_embedded_image_fields() {
    let output: DocumentOutput = serde_json::from_value(json!({
        "document": {
            "document_id": "doc_docx",
            "filename": "fixture.docx",
            "total_pages": 1,
            "metadata": {"source_type": "docx"}
        },
        "pages": [{
            "chunk_id": "chunk_1",
            "doc_title": "fixture.docx",
            "page_number": 1,
            "text": "# Heading",
            "tables": [[["Metric", "Value"]]],
            "html": "<h1>Heading</h1>",
            "embedded_images": [{
                "id": "image_1",
                "relationship_id": "rId1",
                "content_type": "image/png",
                "filename": "image1.png",
                "alt_text": "Diagram"
            }]
        }]
    }))
    .unwrap();

    assert_eq!(
        output.pages[0].embedded_images[0].alt_text.as_deref(),
        Some("Diagram")
    );

    let value = serde_json::to_value(output).unwrap();
    assert_eq!(
        value["pages"][0]["embedded_images"][0]["filename"],
        "image1.png"
    );
}

#[test]
fn document_output_renders_shared_markdown_report() {
    let output: DocumentOutput = serde_json::from_value(json!({
        "document": {
            "document_id": "doc_fixture",
            "filename": "fixture.pdf",
            "total_pages": 2,
            "metadata": {}
        },
        "pages": [
            {
                "chunk_id": "chunk_1",
                "doc_title": "fixture.pdf",
                "page_number": 1,
                "text": "hello",
                "tables": [],
                "summary_notes": ["first note"],
                "summary_topics": ["topic-a"],
                "summary_relevancy": 92
            },
            {
                "chunk_id": "chunk_2",
                "doc_title": "fixture.pdf",
                "page_number": 2,
                "text": "no summary",
                "tables": []
            }
        ]
    }))
    .unwrap();

    assert_eq!(
        output.to_markdown(),
        "# fixture.pdf\n\n## Page 1\n\n### Topics\n- topic-a\n\n### Summary Notes\n- first note\n"
    );
}
