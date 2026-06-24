use summarizer_extraction::Extractor;
use summarizer_types::{DocumentOutput, PipelineConfig};

#[tokio::test]
async fn golden_non_pdf_extraction_matches_python_baselines() {
    for fixture in [
        "fixture_text",
        "fixture_markdown",
        "fixture_table_text",
        "fixture_pptx",
    ] {
        let baseline = read_baseline(fixture);
        let output = Extractor::new()
            .extract_path(
                format!("doc_{fixture}"),
                &input_path(fixture),
                &PipelineConfig {
                    chunk_size: 3000,
                    chunk_overlap: 80,
                    skip_images: true,
                    run_summarization: false,
                    ..PipelineConfig::default()
                },
            )
            .await
            .unwrap();

        assert_eq!(
            output.document.filename, baseline.document.filename,
            "{fixture} filename drifted"
        );
        assert_eq!(
            output.document.total_pages, baseline.document.total_pages,
            "{fixture} page count drifted"
        );
        assert_eq!(
            output.pages.len(),
            baseline.pages.len(),
            "{fixture} page drift"
        );
        for (actual, expected) in output.pages.iter().zip(&baseline.pages) {
            assert_eq!(
                actual.chunk_id, expected.chunk_id,
                "{fixture} chunk id drift"
            );
            let deviation = normalized_char_deviation(&actual.text, &expected.text);
            assert!(
                deviation < 0.02,
                "{fixture} text deviation {deviation:.4} exceeded 2%"
            );
            assert_eq!(actual.tables, expected.tables, "{fixture} table drift");
        }
    }
}

#[tokio::test]
async fn golden_pdf_extraction_text_stays_close_to_python_baseline() {
    if std::env::var("RUN_PDFIUM_TESTS").ok().as_deref() != Some("1") {
        eprintln!("SKIPPED: set RUN_PDFIUM_TESTS=1 to run PDF golden coverage");
        return;
    }

    let baseline = read_baseline("fixture_pdf");
    let output = Extractor::new()
        .extract_path(
            "doc_fixture_pdf".to_string(),
            &input_path("fixture_pdf"),
            &PipelineConfig {
                chunk_size: 3000,
                chunk_overlap: 80,
                skip_images: true,
                run_summarization: false,
                ..PipelineConfig::default()
            },
        )
        .await
        .unwrap();

    assert_eq!(output.document.total_pages, baseline.document.total_pages);
    assert_eq!(output.pages.len(), baseline.pages.len());
    for (actual, expected) in output.pages.iter().zip(&baseline.pages) {
        let deviation = normalized_char_deviation(&actual.text, &expected.text);
        assert!(
            deviation < 0.02,
            "page {:?} text deviation {deviation:.4} exceeded 2%",
            actual.page_number
        );
        assert_eq!(actual.tables, expected.tables);
    }
}

fn read_baseline(name: &str) -> DocumentOutput {
    let path = repo_root()
        .join("backend-rs")
        .join("tests")
        .join("fixtures")
        .join("golden_outputs")
        .join(format!("{name}.json"));
    let contents = std::fs::read_to_string(path).unwrap();
    serde_json::from_str(&contents).unwrap()
}

fn input_path(name: &str) -> std::path::PathBuf {
    let repo = repo_root();
    match name {
        "fixture_text" => repo.join("backend-rs/tests/fixtures/golden/test_document.txt"),
        "fixture_markdown" => repo.join("backend-rs/tests/fixtures/golden/simple.md"),
        "fixture_table_text" => repo.join("backend-rs/tests/fixtures/golden/table.txt"),
        "fixture_pptx" => repo.join("backend-rs/tests/fixtures/golden/test_presentation.pptx"),
        "fixture_pdf" => {
            repo.join("input/ai-transparency-contact-center-summarization-technical-note.pdf")
        }
        other => panic!("unknown fixture {other}"),
    }
}

fn repo_root() -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(3)
        .unwrap()
        .to_path_buf()
}

fn normalized_char_deviation(actual: &str, expected: &str) -> f64 {
    let actual = normalize_text(actual);
    let expected = normalize_text(expected);
    if expected.is_empty() {
        return if actual.is_empty() { 0.0 } else { 1.0 };
    }

    levenshtein(&actual, &expected) as f64 / expected.chars().count() as f64
}

fn normalize_text(text: &str) -> String {
    text.lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .filter(|line| !line.starts_with("Page ") || !line.contains(" of "))
        .filter(|line| !line.contains('©') && !line.to_ascii_lowercase().contains("all rights"))
        .flat_map(|line| line.split_whitespace())
        .collect::<Vec<_>>()
        .join(" ")
}

fn levenshtein(left: &str, right: &str) -> usize {
    let right_chars: Vec<char> = right.chars().collect();
    let mut costs: Vec<usize> = (0..=right_chars.len()).collect();

    for (left_index, left_char) in left.chars().enumerate() {
        let mut previous = costs[0];
        costs[0] = left_index + 1;
        for (right_index, right_char) in right_chars.iter().enumerate() {
            let current = costs[right_index + 1];
            costs[right_index + 1] = if left_char == *right_char {
                previous
            } else {
                1 + previous.min(current).min(costs[right_index])
            };
            previous = current;
        }
    }

    *costs.last().unwrap()
}
