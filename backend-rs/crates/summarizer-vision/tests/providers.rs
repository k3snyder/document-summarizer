use summarizer_vision::{
    CliVisionProvider, GeminiVisionProvider, OpenAiCompatibleVisionProvider, VisionPage,
    VisionProvider,
};
use wiremock::matchers::{body_string_contains, header, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
use std::time::Duration;

#[tokio::test]
async fn openai_compatible_provider_classifies_and_extracts() {
    let server = MockServer::start().await;
    mock_openai(&server, "substantive visual content", "YES").await;
    mock_openai(
        &server,
        "Analyze this document page image",
        "A chart shows rising volume.",
    )
    .await;
    let provider = OpenAiCompatibleVisionProvider::new(server.uri(), None, "vision-model");
    let page = sample_page();

    let classification = provider.classify(&page).await.unwrap();
    assert!(classification.has_graphics);
    assert_eq!(classification.page_number, 2);

    let extraction = provider.extract(&page).await.unwrap();
    assert_eq!(
        extraction.image_text.as_deref(),
        Some("A chart shows rising volume.")
    );
}

#[tokio::test]
async fn openai_compatible_provider_can_disable_llama_cpp_thinking() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .and(body_string_contains("Analyze this document page image"))
        .and(body_string_contains("\"enable_thinking\":false"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "choices": [{"message": {"content": "Visible text output."}}]
        })))
        .expect(1)
        .mount(&server)
        .await;

    let provider = OpenAiCompatibleVisionProvider::new(server.uri(), None, "vision-model")
        .with_llama_cpp_options();

    let extraction = provider.extract(&sample_page()).await.unwrap();
    assert_eq!(
        extraction.image_text.as_deref(),
        Some("Visible text output.")
    );
}

#[tokio::test]
async fn openai_compatible_provider_caps_completion_tokens_by_call_type() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .and(body_string_contains("substantive visual content"))
        .and(body_string_contains("\"max_tokens\":16"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "choices": [{"message": {"content": "YES"}}]
        })))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .and(body_string_contains("Analyze this document page image"))
        .and(body_string_contains("\"max_tokens\":2048"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "choices": [{"message": {"content": "Capped visual description."}}]
        })))
        .expect(1)
        .mount(&server)
        .await;
    let provider = OpenAiCompatibleVisionProvider::new(server.uri(), None, "vision-model");
    let page = sample_page();

    assert!(provider.classify(&page).await.unwrap().has_graphics);
    assert_eq!(
        provider.extract(&page).await.unwrap().image_text.as_deref(),
        Some("Capped visual description.")
    );
}

#[tokio::test]
async fn openai_compatible_provider_times_out_slow_responses() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_delay(Duration::from_secs(1))
                .set_body_json(serde_json::json!({
                    "choices": [{"message": {"content": "late response"}}]
                })),
        )
        .expect(1)
        .mount(&server)
        .await;

    let provider = OpenAiCompatibleVisionProvider::new(server.uri(), None, "vision-model")
        .with_http_timeout(Duration::from_millis(50));
    let error = provider.classify(&sample_page()).await.unwrap_err();

    assert!(error.to_string().contains("timed out"));
}

#[tokio::test]
async fn openai_compatible_provider_reports_status_error_body() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .respond_with(ResponseTemplate::new(429).set_body_string("quota exceeded for vision"))
        .expect(1)
        .mount(&server)
        .await;

    let provider = OpenAiCompatibleVisionProvider::new(server.uri(), None, "vision-model");
    let error = provider.classify(&sample_page()).await.unwrap_err();
    let message = error.to_string();

    assert!(message.contains("HTTP 429"));
    assert!(message.contains("quota exceeded for vision"));
}

#[tokio::test]
async fn gemini_provider_classifies_and_extracts() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1beta/models/gemini-test:generateContent"))
        .and(header("x-goog-api-key", "test-key"))
        .and(body_string_contains("substantive visual content"))
        .respond_with(gemini_response("YES"))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/v1beta/models/gemini-test:generateContent"))
        .and(header("x-goog-api-key", "test-key"))
        .and(body_string_contains("Analyze this document page image"))
        .respond_with(gemini_response("Gemini visual description."))
        .expect(1)
        .mount(&server)
        .await;
    let provider =
        GeminiVisionProvider::new(server.uri(), Some("test-key".to_string()), "gemini-test");
    let page = sample_page();

    assert!(provider.classify(&page).await.unwrap().has_graphics);
    assert_eq!(
        provider.extract(&page).await.unwrap().image_text.as_deref(),
        Some("Gemini visual description.")
    );
}

#[tokio::test]
async fn gemini_provider_times_out_slow_responses() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1beta/models/gemini-test:generateContent"))
        .and(header("x-goog-api-key", "test-key"))
        .respond_with(ResponseTemplate::new(200).set_delay(Duration::from_secs(1)))
        .expect(1)
        .mount(&server)
        .await;

    let provider =
        GeminiVisionProvider::new(server.uri(), Some("test-key".to_string()), "gemini-test")
            .with_http_timeout(Duration::from_millis(50));
    let error = provider.classify(&sample_page()).await.unwrap_err();

    assert!(error.to_string().contains("timed out"));
}

#[tokio::test]
async fn gemini_provider_truncates_status_error_body() {
    let server = MockServer::start().await;
    let body = format!("{}tail-marker", "x".repeat(3000));
    Mock::given(method("POST"))
        .and(path("/v1beta/models/gemini-test:generateContent"))
        .and(header("x-goog-api-key", "test-key"))
        .respond_with(ResponseTemplate::new(500).set_body_string(body))
        .expect(1)
        .mount(&server)
        .await;

    let provider =
        GeminiVisionProvider::new(server.uri(), Some("test-key".to_string()), "gemini-test");
    let error = provider.classify(&sample_page()).await.unwrap_err();
    let message = error.to_string();

    assert!(message.contains("HTTP 500"));
    assert!(message.contains("[truncated]"));
    assert!(!message.contains("tail-marker"));
}

#[tokio::test]
async fn cli_provider_classifies_and_extracts() {
    let (executable, args) = generic_cli_command();
    let provider = CliVisionProvider::new(executable)
        .with_args(args)
        .with_timeout_seconds(5);
    let page = sample_page();

    assert!(provider.classify(&page).await.unwrap().has_graphics);
    assert_eq!(
        provider.extract(&page).await.unwrap().image_text.as_deref(),
        Some("CLI visual description")
    );
}

#[cfg(unix)]
#[tokio::test]
async fn codex_cli_provider_uses_exec_json_and_image_file() {
    let dir = tempfile::tempdir().unwrap();
    let capture = dir.path().join("args.txt");
    let script_path = dir.path().join("fake-codex");
    std::fs::write(
        &script_path,
        format!(
            r#"#!/bin/sh
printf '%s\n' "$@" > "{}"
while IFS= read -r _line; do :; done
printf '%s\n' '{{"type":"turn.started","context":{{"model":"gpt-codex-vision-test"}}}}'
printf '%s\n' '{{"type":"item.completed","item":{{"type":"agent_message","text":"Codex visual description"}}}}'
"#,
            capture.display()
        ),
    )
    .unwrap();
    let mut permissions = std::fs::metadata(&script_path).unwrap().permissions();
    permissions.set_mode(0o755);
    std::fs::set_permissions(&script_path, permissions).unwrap();

    let provider =
        CliVisionProvider::codex(script_path.to_string_lossy().to_string()).with_timeout_seconds(5);
    let page = sample_page();

    assert!(provider.classify(&page).await.unwrap().has_graphics);
    assert_eq!(
        provider.extract(&page).await.unwrap().image_text.as_deref(),
        Some("Codex visual description")
    );
    assert_eq!(
        provider.reported_model().as_deref(),
        Some("gpt-codex-vision-test")
    );

    let args = std::fs::read_to_string(capture).unwrap();
    assert!(args.contains("exec"));
    assert!(args.contains("--json"));
    assert!(args.contains("--image"));
    assert!(args.contains("-C"));
}

#[cfg(unix)]
#[tokio::test]
async fn grok_cli_provider_uses_prompt_file_json_output_and_image_reference() {
    let dir = tempfile::tempdir().unwrap();
    let capture = dir.path().join("args.txt");
    let prompt_capture = dir.path().join("prompt.txt");
    let script_path = dir.path().join("fake-grok");
    std::fs::write(
        &script_path,
        format!(
            r#"#!/bin/sh
printf '%s\n' "$@" > "{}"
printf 'HOME=%s\nGROK_HOME=%s\nGROK_CLAUDE_MCPS_ENABLED=%s\nGROK_CURSOR_MCPS_ENABLED=%s\nCMUX_GROK_HOOKS_DISABLED=%s\n' "$HOME" "$GROK_HOME" "$GROK_CLAUDE_MCPS_ENABLED" "$GROK_CURSOR_MCPS_ENABLED" "$CMUX_GROK_HOOKS_DISABLED" >> "{}"
prompt_file=''
while [ "$#" -gt 0 ]; do
  if [ "$1" = "--prompt-file" ]; then
    shift
    prompt_file="$1"
  fi
  shift
done
cp "$prompt_file" "{}"
input=$(cat "$prompt_file")
case "$input" in
  *"substantive visual content"*) text='YES' ;;
  *"3 independent vision extractions"*) text='Grok synthesis' ;;
  *) text='Grok visual description' ;;
esac
printf '{{"text":"%s","thought":"hidden"}}\n' "$text"
"#,
            capture.display(),
            capture.display(),
            prompt_capture.display()
        ),
    )
    .unwrap();
    let mut permissions = std::fs::metadata(&script_path).unwrap().permissions();
    permissions.set_mode(0o755);
    std::fs::set_permissions(&script_path, permissions).unwrap();

    let provider =
        CliVisionProvider::grok(script_path.to_string_lossy().to_string()).with_timeout_seconds(5);
    let page = sample_page();

    assert!(provider.classify(&page).await.unwrap().has_graphics);
    assert_eq!(
        provider.extract(&page).await.unwrap().image_text.as_deref(),
        Some("Grok visual description")
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
    let prompt = std::fs::read_to_string(prompt_capture).unwrap();
    assert!(prompt.contains("@page_2.png"));
}

async fn mock_openai(server: &MockServer, body_contains: &str, content: &str) {
    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .and(body_string_contains(body_contains))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "choices": [{"message": {"content": content}}]
        })))
        .expect(1)
        .mount(server)
        .await;
}

fn gemini_response(content: &str) -> ResponseTemplate {
    ResponseTemplate::new(200).set_body_json(serde_json::json!({
        "candidates": [{
            "content": {
                "parts": [{"text": content}]
            }
        }]
    }))
}

#[cfg(unix)]
fn generic_cli_command() -> (&'static str, Vec<String>) {
    (
        "sh",
        vec![
            "-c".to_string(),
            r#"input=$(cat)
case "$input" in
  *"substantive visual content"*) printf 'YES' ;;
  *) printf 'CLI visual description' ;;
esac"#
                .to_string(),
        ],
    )
}

#[cfg(windows)]
fn generic_cli_command() -> (&'static str, Vec<String>) {
    let script = r#"$inputText = [Console]::In.ReadToEnd(); if ($inputText -like '*substantive visual content*') { [Console]::Write('YES') } else { [Console]::Write('CLI visual description') }"#;
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

fn sample_page() -> VisionPage {
    VisionPage {
        page_number: 2,
        chunk_id: "chunk_2".to_string(),
        image_base64: "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/x8AAwMCAO+/p9sAAAAASUVORK5CYII=".to_string(),
    }
}
