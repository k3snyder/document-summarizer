use summarizer_summarization::{
    CliSummarizer, OpenAiCompatibleSummarizer, SummarizationBudgetExhaustReason,
    SummarizationOptions, Summarizer,
};
use summarizer_types::{PageOutput, SummarizerMode};
use wiremock::matchers::{body_string_contains, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
use std::time::Duration;

#[tokio::test]
async fn openai_compatible_summarizer_runs_quality_loop_and_topics() {
    let server = MockServer::start().await;

    mock_chat(&server, "accurately summarizing", "* Strong note", 13).await;
    mock_chat(&server, "Compare the following original text", "91%", 7).await;
    mock_chat(
        &server,
        "Analyze the topic categories",
        "Operations, Quality assurance",
        5,
    )
    .await;

    let summarizer = OpenAiCompatibleSummarizer::new(server.uri(), None, "summary-model");
    let result = summarizer
        .summarize_page(
            &sample_page(),
            SummarizationOptions::new(SummarizerMode::Full),
        )
        .await
        .unwrap();

    assert_eq!(result.attempts_used, 1);
    assert_eq!(result.tokens, 25);
    assert_eq!(
        result.page.summary_notes,
        Some(vec!["Strong note".to_string()])
    );
    assert_eq!(
        result.page.summary_topics,
        Some(vec![
            "Operations".to_string(),
            "Quality assurance".to_string()
        ])
    );
    assert_eq!(result.page.summary_relevancy, Some(91));
    assert_eq!(result.page.summary_quality_validated, Some(true));
}

#[tokio::test]
async fn openai_compatible_summarizer_supports_topics_only_mode() {
    let server = MockServer::start().await;
    mock_chat(
        &server,
        "Analyze the topic categories",
        "Customer experience, Compliance",
        9,
    )
    .await;

    let summarizer = OpenAiCompatibleSummarizer::new(server.uri(), None, "summary-model");
    let result = summarizer
        .summarize_page(
            &sample_page(),
            SummarizationOptions::new(SummarizerMode::TopicsOnly),
        )
        .await
        .unwrap();

    assert_eq!(result.attempts_used, 1);
    assert_eq!(result.tokens, 9);
    assert_eq!(result.page.summary_notes, None);
    assert_eq!(
        result.page.summary_topics,
        Some(vec![
            "Customer experience".to_string(),
            "Compliance".to_string()
        ])
    );
    assert_eq!(result.page.summary_relevancy, Some(0));
}

#[tokio::test]
async fn openai_compatible_summarizer_stops_when_token_budget_exhausted() {
    let server = MockServer::start().await;
    mock_chat(&server, "accurately summarizing", "* Budgeted note", 1500).await;

    let summarizer =
        OpenAiCompatibleSummarizer::new(server.uri(), None, "summary-model").with_budget(1000, 300);
    let result = summarizer
        .summarize_page(
            &sample_page(),
            SummarizationOptions::new(SummarizerMode::Full),
        )
        .await
        .unwrap();

    assert_eq!(result.attempts_used, 1);
    assert_eq!(result.tokens, 1500);
    assert_eq!(
        result.budget_exhausted,
        Some(SummarizationBudgetExhaustReason::TokensExceeded)
    );
    assert_eq!(
        result.page.summary_notes,
        Some(vec!["Budgeted note".to_string()])
    );
    assert_eq!(result.page.summary_quality_validated, Some(false));
}

#[tokio::test]
async fn openai_compatible_summarizer_can_disable_llama_cpp_thinking() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .and(body_string_contains("Analyze the topic categories"))
        .and(body_string_contains("\"enable_thinking\":false"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "choices": [{"message": {"content": "Operations"}}],
            "usage": {"total_tokens": 9}
        })))
        .expect(1)
        .mount(&server)
        .await;

    let summarizer = OpenAiCompatibleSummarizer::new(server.uri(), None, "summary-model")
        .with_llama_cpp_options();
    let result = summarizer
        .summarize_page(
            &sample_page(),
            SummarizationOptions::new(SummarizerMode::TopicsOnly),
        )
        .await
        .unwrap();

    assert_eq!(
        result.page.summary_topics,
        Some(vec!["Operations".to_string()])
    );
}

#[tokio::test]
async fn cli_summarizer_runs_notes_and_topics_subprocesses() {
    let (executable, args) = generic_cli_command();
    let summarizer = CliSummarizer::new(executable)
        .with_args(args)
        .with_timeout_seconds(5);

    let result = summarizer
        .summarize_page(
            &sample_page(),
            SummarizationOptions::new(SummarizerMode::Full),
        )
        .await
        .unwrap();

    assert_eq!(
        result.page.summary_notes,
        Some(vec!["CLI note from document".to_string()])
    );
    assert_eq!(
        result.page.summary_topics,
        Some(vec![
            "Process control".to_string(),
            "Customer support".to_string()
        ])
    );
    assert_eq!(result.page.summary_relevancy, None);
    assert_eq!(result.page.summary_quality_validated, Some(false));
    assert_eq!(result.attempts_used, 1);
}

#[tokio::test]
async fn openai_compatible_summarizer_skips_repeated_quality_bands() {
    let server = MockServer::start().await;
    mock_chat_times(&server, "accurately summarizing", "* Repeated note", 1, 2).await;
    mock_chat_times(
        &server,
        "Summarize the following source text",
        "* Repeated note",
        1,
        6,
    )
    .await;
    mock_chat_times(&server, "Compare the following original text", "80%", 1, 8).await;
    mock_chat(&server, "Analyze the topic categories", "Operations", 1).await;

    let summarizer = OpenAiCompatibleSummarizer::new(server.uri(), None, "summary-model");
    let result = summarizer
        .summarize_page(
            &sample_page(),
            SummarizationOptions::new(SummarizerMode::Full),
        )
        .await
        .unwrap();

    assert_eq!(result.attempts_used, 22);
    assert_eq!(result.tokens, 17);
    assert_eq!(result.page.summary_relevancy, Some(80));
    assert_eq!(result.page.summary_quality_validated, Some(false));
}

#[cfg(unix)]
#[tokio::test]
async fn codex_cli_summarizer_uses_exec_json_output() {
    let dir = tempfile::tempdir().unwrap();
    let capture = dir.path().join("args.txt");
    let script_path = dir.path().join("fake-codex");
    std::fs::write(
        &script_path,
        format!(
            r#"#!/bin/sh
printf '%s\n' "$@" >> "{}"
input=$(cat)
case "$input" in
  *"topic categories"*) text='Codex topics, Testing' ;;
  *) text='* Codex note from document' ;;
esac
printf '%s\n' "{{\"type\":\"item.completed\",\"item\":{{\"type\":\"agent_message\",\"text\":\"$text\"}}}}"
"#,
            capture.display()
        ),
    )
    .unwrap();
    let mut permissions = std::fs::metadata(&script_path).unwrap().permissions();
    permissions.set_mode(0o755);
    std::fs::set_permissions(&script_path, permissions).unwrap();

    let summarizer =
        CliSummarizer::codex(script_path.to_string_lossy().to_string()).with_timeout_seconds(5);
    let result = summarizer
        .summarize_page(
            &sample_page(),
            SummarizationOptions::new(SummarizerMode::Full),
        )
        .await
        .unwrap();

    assert_eq!(
        result.page.summary_notes,
        Some(vec!["Codex note from document".to_string()])
    );
    assert_eq!(
        result.page.summary_topics,
        Some(vec!["Codex topics".to_string(), "Testing".to_string()])
    );
    let args = std::fs::read_to_string(capture).unwrap();
    assert!(args.contains("exec"));
    assert!(args.contains("--json"));
    assert!(args.contains("-C"));
}

#[cfg(unix)]
#[tokio::test]
async fn grok_cli_summarizer_uses_prompt_file_json_output() {
    let dir = tempfile::tempdir().unwrap();
    let capture = dir.path().join("args.txt");
    let script_path = dir.path().join("fake-grok");
    std::fs::write(
        &script_path,
        format!(
            r#"#!/bin/sh
printf '%s\n' "$@" >> "{}"
printf 'HOME=%s\nGROK_HOME=%s\nGROK_CLAUDE_MCPS_ENABLED=%s\nGROK_CURSOR_MCPS_ENABLED=%s\nCMUX_GROK_HOOKS_DISABLED=%s\n' "$HOME" "$GROK_HOME" "$GROK_CLAUDE_MCPS_ENABLED" "$GROK_CURSOR_MCPS_ENABLED" "$CMUX_GROK_HOOKS_DISABLED" >> "{}"
prompt_file=''
while [ "$#" -gt 0 ]; do
  if [ "$1" = "--prompt-file" ]; then
    shift
    prompt_file="$1"
  fi
  shift
done
input=$(cat "$prompt_file")
case "$input" in
  *"topic categories"*) text='Grok topics, Testing' ;;
  *) text='* Grok note from document' ;;
esac
printf '{{"text":"%s","thought":"hidden"}}\n' "$text"
"#,
            capture.display(),
            capture.display()
        ),
    )
    .unwrap();
    let mut permissions = std::fs::metadata(&script_path).unwrap().permissions();
    permissions.set_mode(0o755);
    std::fs::set_permissions(&script_path, permissions).unwrap();

    let summarizer =
        CliSummarizer::grok(script_path.to_string_lossy().to_string()).with_timeout_seconds(5);
    let result = summarizer
        .summarize_page(
            &sample_page(),
            SummarizationOptions::new(SummarizerMode::Full),
        )
        .await
        .unwrap();

    assert_eq!(
        result.page.summary_notes,
        Some(vec!["Grok note from document".to_string()])
    );
    assert_eq!(
        result.page.summary_topics,
        Some(vec!["Grok topics".to_string(), "Testing".to_string()])
    );
    let args = std::fs::read_to_string(capture).unwrap();
    assert!(args.contains("--prompt-file"));
    assert!(args.contains("--output-format"));
    assert!(args.contains("json"));
    assert!(args.contains("HOME="));
    assert!(args.contains("grok-home"));
    assert!(args.contains("GROK_HOME="));
    assert!(args.contains("GROK_CLAUDE_MCPS_ENABLED=false"));
    assert!(args.contains("GROK_CURSOR_MCPS_ENABLED=false"));
    assert!(args.contains("CMUX_GROK_HOOKS_DISABLED=1"));
    assert!(!result
        .page
        .summary_notes
        .as_ref()
        .unwrap()
        .iter()
        .any(|note| note.contains("hidden")));
}

#[tokio::test]
async fn openai_compatible_summarizer_times_out_slow_responses() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_delay(Duration::from_secs(1))
                .set_body_json(serde_json::json!({
                    "choices": [{"message": {"content": "* Late note"}}],
                    "usage": {"total_tokens": 1}
                })),
        )
        .expect(1)
        .mount(&server)
        .await;

    let summarizer = OpenAiCompatibleSummarizer::new(server.uri(), None, "summary-model")
        .with_http_timeout(Duration::from_millis(50));
    let error = summarizer
        .summarize_page(
            &sample_page(),
            SummarizationOptions::new(SummarizerMode::Full),
        )
        .await
        .unwrap_err();

    assert!(error.to_string().contains("timed out"));
}

#[tokio::test]
async fn openai_compatible_summarizer_reports_status_error_body() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .respond_with(
            ResponseTemplate::new(502)
                .set_body_string("{\"error\":\"upstream model unavailable\"}"),
        )
        .expect(1)
        .mount(&server)
        .await;

    let summarizer = OpenAiCompatibleSummarizer::new(server.uri(), None, "summary-model");
    let error = summarizer
        .summarize_page(
            &sample_page(),
            SummarizationOptions::new(SummarizerMode::Full),
        )
        .await
        .unwrap_err();
    let message = error.to_string();

    assert!(message.contains("HTTP 502"));
    assert!(message.contains("upstream model unavailable"));
}

#[tokio::test]
async fn openai_compatible_summarizer_supports_insight_mode() {
    let server = MockServer::start().await;
    mock_chat(&server, "accurately summarizing", "* Base note", 10).await;
    mock_chat(&server, "Compare the following original text", "94%", 4).await;
    mock_chat(&server, "Analyze the topic categories", "Operations", 3).await;
    mock_chat(
        &server,
        "additional high-value insights",
        "* Extra insight",
        8,
    )
    .await;

    let summarizer = OpenAiCompatibleSummarizer::new(server.uri(), None, "summary-model");
    let result = summarizer
        .summarize_page(
            &sample_page(),
            SummarizationOptions {
                mode: SummarizerMode::Full,
                detailed_extraction: false,
                insight_mode: true,
            },
        )
        .await
        .unwrap();

    assert_eq!(
        result.page.summary_notes,
        Some(vec!["Base note".to_string(), "Extra insight".to_string()])
    );
    assert_eq!(result.page.summary_relevancy, Some(94));
    assert_eq!(result.page.summary_quality_validated, Some(true));
    assert_eq!(result.attempts_used, 2);
    assert_eq!(result.tokens, 25);
}

#[tokio::test]
async fn openai_compatible_summarizer_supports_detailed_extraction_mode() {
    let server = MockServer::start().await;
    mock_chat_times(&server, "accurately summarizing", "* Pass note", 10, 3).await;
    mock_chat_times(&server, "Compare the following original text", "90%", 4, 3).await;
    mock_chat_times(&server, "Analyze the topic categories", "Operations", 3, 4).await;
    mock_chat(
        &server,
        "three independent note extractions",
        "* Synthesized note",
        9,
    )
    .await;

    let summarizer = OpenAiCompatibleSummarizer::new(server.uri(), None, "summary-model");
    let result = summarizer
        .summarize_page(
            &sample_page(),
            SummarizationOptions {
                mode: SummarizerMode::Full,
                detailed_extraction: true,
                insight_mode: false,
            },
        )
        .await
        .unwrap();

    assert_eq!(
        result.page.summary_notes,
        Some(vec!["Synthesized note".to_string()])
    );
    assert_eq!(
        result.page.summary_notes_1,
        Some(vec!["Pass note".to_string()])
    );
    assert_eq!(result.page.summary_relevancy, Some(90));
    assert_eq!(result.page.summary_quality_validated, Some(true));
    assert_eq!(result.attempts_used, 4);
    assert_eq!(result.tokens, 63);
}

async fn mock_chat(server: &MockServer, body_contains: &str, content: &str, total_tokens: usize) {
    mock_chat_times(server, body_contains, content, total_tokens, 1).await;
}

#[cfg(unix)]
fn generic_cli_command() -> (&'static str, Vec<String>) {
    (
        "sh",
        vec![
            "-c".to_string(),
            r#"input=$(cat)
case "$input" in
  *"topic categories"*) printf 'Process control, Customer support' ;;
  *) printf '* CLI note from document' ;;
esac"#
                .to_string(),
        ],
    )
}

#[cfg(windows)]
fn generic_cli_command() -> (&'static str, Vec<String>) {
    let script = r#"$inputText = [Console]::In.ReadToEnd(); if ($inputText -like '*topic categories*') { [Console]::Write('Process control, Customer support') } else { [Console]::Write('* CLI note from document') }"#;
    (
        "powershell",
        vec![
            "-NoProfile".to_string(),
            "-ExecutionPolicy".to_string(),
            "Bypass".to_string(),
            "-Command".to_string(),
            script.to_string(),
        ],
    )
}

async fn mock_chat_times(
    server: &MockServer,
    body_contains: &str,
    content: &str,
    total_tokens: usize,
    expected_calls: u64,
) {
    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .and(body_string_contains(body_contains))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "choices": [{"message": {"content": content}}],
            "usage": {"total_tokens": total_tokens}
        })))
        .expect(expected_calls)
        .mount(server)
        .await;
}

fn sample_page() -> PageOutput {
    PageOutput {
        chunk_id: "chunk_1".to_string(),
        doc_title: "fixture.txt".to_string(),
        page_number: Some(1),
        text: "This page explains operational quality controls.".to_string(),
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
    }
}
