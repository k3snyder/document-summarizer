use summarizer_extraction::{extract_tables_from_text, extract_text_document, split_recursive};
use summarizer_types::PipelineConfig;

#[test]
fn split_recursive_keeps_overlap() {
    let chunks = split_recursive("alpha beta gamma delta epsilon zeta", 18, 5);

    assert!(chunks.len() > 1);
    assert!(chunks[0].contains("alpha"));
    assert!(chunks[1].contains("gamma") || chunks[1].contains("delta"));
}

#[test]
fn table_heuristic_preserves_multiple_tables() {
    let text = "\
Name  Value  Status
Alpha  10  Open
Beta  20  Closed

Body paragraph without columns.

Left | Right
A | B
C | D";

    let tables = extract_tables_from_text(text);

    assert_eq!(tables.len(), 2);
    assert_eq!(tables[0][0], ["Name", "Value", "Status"]);
    assert_eq!(tables[0][2], ["Beta", "20", "Closed"]);
    assert_eq!(tables[1][0], ["Left", "Right"]);
    assert_eq!(tables[1][2], ["C", "D"]);
}

#[test]
fn text_document_uses_canonical_output_shape() {
    let config = PipelineConfig {
        chunk_size: 20,
        chunk_overlap: 0,
        ..PipelineConfig::default()
    };
    let output = extract_text_document(
        "doc_test".to_string(),
        "test.txt".to_string(),
        "hello world",
        &config,
    );

    assert_eq!(output.document.total_pages, 1);
    assert_eq!(output.pages[0].tables, Vec::<Vec<Vec<String>>>::new());
    assert_eq!(output.pages[0].image_base64, None);
}
