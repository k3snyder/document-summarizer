use std::process::{Command, Output};

use serde_json::Value;

fn bin() -> &'static str {
    env!("CARGO_BIN_EXE_summarizer-cli")
}

#[test]
fn txt_extract_only_writes_output_and_manifest() {
    let temp = tempfile::tempdir().unwrap();
    let input = temp.path().join("sample.txt");
    let output = temp.path().join("custom_output.json");
    std::fs::write(&input, "Alpha\nBeta\n").unwrap();

    let completed = command()
        .arg(&input)
        .args([
            "--env-providers",
            "--config-json",
            r#"{"extract_only":true}"#,
            "--output",
        ])
        .arg(&output)
        .output()
        .unwrap();

    assert_success(&completed);
    assert!(output.is_file());
    let manifest = stdout_json(&completed);
    assert_eq!(manifest["status"], "completed");
    assert_eq!(manifest["output_json_path"], output.display().to_string());
    assert_eq!(manifest["document"]["filename"], "sample.txt");

    let document: summarizer_types::DocumentOutput =
        serde_json::from_slice(&std::fs::read(&output).unwrap()).unwrap();
    assert_eq!(document.document.filename, "sample.txt");
    assert_eq!(document.pages.len(), 1);
}

#[test]
fn config_json_merge_keeps_desktop_defaults() {
    let temp = tempfile::tempdir().unwrap();
    let input = temp.path().join("sample.txt");
    std::fs::write(&input, "Alpha\n").unwrap();

    let completed = command()
        .arg(&input)
        .args([
            "--config-json",
            r#"{"vision_skip_classification":true}"#,
            "--print-config",
        ])
        .output()
        .unwrap();

    assert_success(&completed);
    let config = stdout_json(&completed);
    assert_eq!(config["vision_skip_classification"], true);
    assert_eq!(config["vision_mode"], "codex");
    assert_eq!(config["summarizer_provider"], "codex");
}

#[test]
fn set_values_apply_after_config_json() {
    let temp = tempfile::tempdir().unwrap();
    let input = temp.path().join("sample.txt");
    std::fs::write(&input, "Alpha\n").unwrap();

    let completed = command()
        .arg(&input)
        .args([
            "--config-json",
            r#"{"extract_only":false}"#,
            "--set",
            "extract_only=true",
            "--print-config",
        ])
        .output()
        .unwrap();

    assert_success(&completed);
    let config = stdout_json(&completed);
    assert_eq!(config["extract_only"], true);
}

#[test]
fn missing_input_exits_environment_error() {
    let temp = tempfile::tempdir().unwrap();
    let missing = temp.path().join("missing.txt");

    let completed = command().arg(&missing).output().unwrap();

    assert_eq!(completed.status.code(), Some(3));
    let manifest = stdout_json(&completed);
    assert_eq!(manifest["status"], "failed");
    assert!(manifest["error"]
        .as_str()
        .unwrap()
        .contains("Input file not found"));
}

#[test]
fn non_object_config_json_exits_usage_error() {
    let temp = tempfile::tempdir().unwrap();
    let input = temp.path().join("sample.txt");
    std::fs::write(&input, "Alpha\n").unwrap();

    let completed = command()
        .arg(&input)
        .args(["--config-json", "[1,2,3]"])
        .output()
        .unwrap();

    assert_eq!(completed.status.code(), Some(2));
    assert!(
        String::from_utf8_lossy(&completed.stderr).contains("config JSON must be a JSON object")
    );
}

#[test]
fn malformed_settings_file_exits_environment_error() {
    let temp = tempfile::tempdir().unwrap();
    let input = temp.path().join("sample.txt");
    let settings = temp.path().join("settings.json");
    std::fs::write(&input, "Alpha\n").unwrap();
    std::fs::write(&settings, "{not json").unwrap();

    let completed = command()
        .arg(&input)
        .args(["--config-json", r#"{"extract_only":true}"#, "--settings"])
        .arg(&settings)
        .output()
        .unwrap();

    assert_eq!(completed.status.code(), Some(3));
    let manifest = stdout_json(&completed);
    assert_eq!(manifest["status"], "failed");
    assert!(manifest["error"]
        .as_str()
        .unwrap()
        .contains("Invalid settings JSON"));
}

#[test]
fn pdf_with_missing_explicit_pdfium_exits_environment_error() {
    let temp = tempfile::tempdir().unwrap();
    let input = temp.path().join("sample.pdf");
    let missing = temp.path().join("missing-pdfium");
    std::fs::write(&input, b"%PDF-1.7\n").unwrap();

    let completed = command()
        .arg(&input)
        .args([
            "--env-providers",
            "--config-json",
            r#"{"extract_only":true}"#,
            "--pdfium",
        ])
        .arg(&missing)
        .output()
        .unwrap();

    assert_eq!(completed.status.code(), Some(3));
    let manifest = stdout_json(&completed);
    assert_eq!(manifest["status"], "failed");
    let error = manifest["error"].as_str().unwrap();
    assert!(error.contains("Could not resolve PDFium library"));
    assert!(error.contains(&missing.display().to_string()));
}

#[test]
fn doctor_without_input_can_pass_for_extract_only_config() {
    let completed = command()
        .args([
            "--doctor",
            "--env-providers",
            "--config-json",
            r#"{"extract_only":true,"vision_mode":"none","run_summarization":false}"#,
        ])
        .output()
        .unwrap();

    assert_success(&completed);
    let report = stdout_json(&completed);
    assert_eq!(report["doctor"], true);
    assert_eq!(report["ok"], true);
    assert!(report["checks"]
        .as_array()
        .unwrap()
        .iter()
        .any(|check| check["name"] == "pdfium" && check["status"] == "skip"));
}

#[test]
fn doctor_reports_dead_llama_cpp_endpoint() {
    let completed = command()
        .env("LLAMA_CPP_BASE_URL", "http://127.0.0.1:1/v1")
        .args([
            "--doctor",
            "--env-providers",
            "--config-json",
            r#"{"vision_mode":"none","summarizer_provider":"llama_cpp"}"#,
        ])
        .output()
        .unwrap();

    assert_eq!(completed.status.code(), Some(1));
    let report = stdout_json(&completed);
    assert_eq!(report["ok"], false);
    let checks = report["checks"].as_array().unwrap();
    let llama = checks
        .iter()
        .find(|check| check["name"] == "provider:llama_cpp")
        .unwrap();
    assert_eq!(llama["status"], "fail");
    assert!(llama["detail"]
        .as_str()
        .unwrap()
        .contains("http://127.0.0.1:1/v1/models"));
}

#[test]
fn estimate_txt_reports_structure_without_writing_output() {
    let temp = tempfile::tempdir().unwrap();
    let input = temp.path().join("sample.txt");
    let output = temp.path().join("sample_output.json");
    std::fs::write(&input, "Alpha\n\nBeta\n").unwrap();

    let completed = command()
        .arg(&input)
        .args([
            "--estimate",
            "--env-providers",
            "--config-json",
            r#"{"extract_only":true,"chunk_size":5,"chunk_overlap":0}"#,
        ])
        .output()
        .unwrap();

    assert_success(&completed);
    assert!(!output.exists());
    let report = stdout_json(&completed);
    assert_eq!(report["estimate"], true);
    assert_eq!(report["stages"]["extraction"], true);
    assert_eq!(report["stages"]["vision"], false);
    assert_eq!(report["stages"]["summarization"], false);
    assert!(report["pages"].as_u64().unwrap() >= 1);
}

#[test]
fn page_range_limits_txt_chunks_and_keeps_original_numbering() {
    let temp = tempfile::tempdir().unwrap();
    let input = temp.path().join("sample.txt");
    let output = temp.path().join("sample_output.json");
    std::fs::write(&input, "Alpha. Beta. Gamma. Delta.").unwrap();

    let completed = command()
        .arg(&input)
        .args([
            "--env-providers",
            "--config-json",
            r#"{"extract_only":true,"chunk_size":7,"chunk_overlap":0,"page_range":"2-3"}"#,
            "--output",
        ])
        .arg(&output)
        .output()
        .unwrap();

    assert_success(&completed);
    let document: summarizer_types::DocumentOutput =
        serde_json::from_slice(&std::fs::read(&output).unwrap()).unwrap();
    assert_eq!(document.document.total_pages, 4);
    assert_eq!(document.pages.len(), 2);
    assert_eq!(document.pages[0].page_number, Some(2));
    assert_eq!(document.pages[1].page_number, Some(3));
}

#[test]
fn estimate_respects_page_range() {
    let temp = tempfile::tempdir().unwrap();
    let input = temp.path().join("sample.txt");
    std::fs::write(&input, "Alpha. Beta. Gamma. Delta.").unwrap();

    let completed = command()
        .arg(&input)
        .args([
            "--estimate",
            "--env-providers",
            "--config-json",
            r#"{"extract_only":true,"chunk_size":7,"chunk_overlap":0,"page_range":"2-3"}"#,
        ])
        .output()
        .unwrap();

    assert_success(&completed);
    let report = stdout_json(&completed);
    assert_eq!(report["pages"], 2);
}

fn command() -> Command {
    let mut command = Command::new(bin());
    command.env_remove("SUMMARIZER_PDFIUM");
    command
}

fn stdout_json(output: &Output) -> Value {
    let stdout = String::from_utf8(output.stdout.clone()).unwrap();
    serde_json::from_str(stdout.trim()).unwrap()
}

fn assert_success(output: &Output) {
    assert!(
        output.status.success(),
        "status={:?}\nstdout={}\nstderr={}",
        output.status.code(),
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}
