use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use summarizer_cli_util::resolve_cli_executable_with_extra_dirs;

use crate::{
    CliRuntimeConfig, GeminiProviderConfig, HttpProviderConfig, LlamaCppProviderConfig,
    OllamaProviderConfig, PipelineProviderConfig, DEFAULT_CLI_RETRIES,
};

const DEFAULT_CODEX_MODEL: &str = "gpt-5.6-terra";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OpenAiSettings {
    pub base_url: String,
    pub api_key: String,
    pub model: String,
    pub model_2: String,
    pub model_3: String,
    pub vision_model: String,
}

impl Default for OpenAiSettings {
    fn default() -> Self {
        Self {
            base_url: "https://api.openai.com/v1".to_string(),
            api_key: String::new(),
            model: "gpt-4.1-mini".to_string(),
            model_2: "gpt-4.1-mini".to_string(),
            model_3: "gpt-4.1-mini".to_string(),
            vision_model: "gpt-4.1-mini".to_string(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LlamaCppSettings {
    pub base_url: String,
    pub vision_base_url: String,
    pub api_key: String,
    pub model: String,
    pub model_2: String,
    pub model_3: String,
    pub vision_model: String,
}

impl Default for LlamaCppSettings {
    fn default() -> Self {
        Self {
            base_url: "http://localhost:11440/v1".to_string(),
            vision_base_url: "http://localhost:11439/v1".to_string(),
            api_key: String::new(),
            model: "model.gguf".to_string(),
            model_2: "model.gguf".to_string(),
            model_3: "model.gguf".to_string(),
            vision_model: "model.gguf".to_string(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OllamaSettings {
    pub openai_base_url: String,
    pub api_key: String,
    pub model: String,
    pub model_2: String,
    pub model_3: String,
    pub vision_model: String,
}

impl Default for OllamaSettings {
    fn default() -> Self {
        Self {
            openai_base_url: "http://localhost:11434/v1".to_string(),
            api_key: String::new(),
            model: "gemma4:12b-it-qat".to_string(),
            model_2: "gemma4:12b-it-qat".to_string(),
            model_3: "gemma4:12b-it-qat".to_string(),
            vision_model: "llava".to_string(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CliSettings {
    pub executable: String,
    pub args: String,
    /// Model ID passed to the CLI (`--model <id>`). An empty string lets the
    /// CLI use its configured/default model. Currently wired for Codex only;
    /// ignored for Claude, Grok, and Copilot.
    #[serde(default)]
    pub model: String,
    #[serde(default = "default_reasoning_effort")]
    pub reasoning_effort: String,
    pub timeout_seconds: u64,
}

impl CliSettings {
    pub fn new(executable: &str) -> Self {
        Self {
            executable: executable.to_string(),
            args: String::new(),
            model: String::new(),
            reasoning_effort: default_reasoning_effort(),
            timeout_seconds: 600,
        }
    }
}

fn default_reasoning_effort() -> String {
    "medium".to_string()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderSettings {
    pub openai: OpenAiSettings,
    pub llama_cpp: LlamaCppSettings,
    pub ollama: OllamaSettings,
    #[serde(
        default = "default_codex_settings",
        deserialize_with = "deserialize_codex_settings"
    )]
    pub codex: CliSettings,
    #[serde(default = "default_claude_settings")]
    pub claude: CliSettings,
    #[serde(default = "default_grok_settings")]
    pub grok: CliSettings,
    #[serde(default = "default_copilot_settings")]
    pub copilot: CliSettings,
}

impl Default for ProviderSettings {
    fn default() -> Self {
        Self {
            openai: OpenAiSettings::default(),
            llama_cpp: LlamaCppSettings::default(),
            ollama: OllamaSettings::default(),
            codex: default_codex_settings(),
            claude: default_claude_settings(),
            grok: default_grok_settings(),
            copilot: default_copilot_settings(),
        }
    }
}

fn default_codex_settings() -> CliSettings {
    let mut settings = CliSettings::new("codex");
    settings.model = DEFAULT_CODEX_MODEL.to_string();
    settings
}

fn deserialize_codex_settings<'de, D>(deserializer: D) -> Result<CliSettings, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let value = serde_json::Value::deserialize(deserializer)?;
    let model_is_missing = value.get("model").is_none();
    let mut settings: CliSettings =
        serde_json::from_value(value).map_err(serde::de::Error::custom)?;
    if model_is_missing {
        settings.model = DEFAULT_CODEX_MODEL.to_string();
    }
    Ok(settings)
}

fn default_claude_settings() -> CliSettings {
    CliSettings::new("claude")
}

fn default_grok_settings() -> CliSettings {
    CliSettings::new("grok")
}

fn default_copilot_settings() -> CliSettings {
    CliSettings::new("copilot")
}

#[derive(Debug, Default, Deserialize)]
pub struct ProviderSettingsFile {
    #[serde(default)]
    pub providers: ProviderSettings,
}

pub fn load_provider_config(path: &Path) -> Result<Option<PipelineProviderConfig>, String> {
    if !path.exists() {
        return Ok(None);
    }
    let contents = std::fs::read_to_string(path)
        .map_err(|err| format!("Could not read settings file {}: {err}", path.display()))?;
    let settings: ProviderSettingsFile = serde_json::from_str(&contents)
        .map_err(|err| format!("Invalid settings JSON {}: {err}", path.display()))?;
    Ok(Some(provider_config_from_settings(&settings.providers)))
}

pub fn provider_config_from_settings(providers: &ProviderSettings) -> PipelineProviderConfig {
    let openai_model = setting_or(&providers.openai.model, "gpt-4.1-mini");
    let llama_model = setting_or(&providers.llama_cpp.model, "model.gguf");
    let ollama_model = setting_or(&providers.ollama.model, "gemma4:12b-it-qat");

    PipelineProviderConfig {
        openai: HttpProviderConfig {
            base_url: setting_or(&providers.openai.base_url, "https://api.openai.com/v1"),
            api_key: optional_setting(&providers.openai.api_key),
            model: openai_model.clone(),
            model_2: setting_or(&providers.openai.model_2, &openai_model),
            model_3: setting_or(&providers.openai.model_3, &openai_model),
            vision_model: setting_or(&providers.openai.vision_model, "gpt-4.1-mini"),
        },
        llama_cpp: LlamaCppProviderConfig {
            base_url: setting_or(&providers.llama_cpp.base_url, "http://localhost:11440/v1"),
            vision_base_url: setting_or(
                fallback_url(
                    &providers.llama_cpp.vision_base_url,
                    &providers.llama_cpp.base_url,
                ),
                "http://localhost:11440/v1",
            ),
            api_key: optional_setting(&providers.llama_cpp.api_key),
            model: llama_model.clone(),
            model_2: setting_or(&providers.llama_cpp.model_2, &llama_model),
            model_3: setting_or(&providers.llama_cpp.model_3, &llama_model),
            vision_model: setting_or(&providers.llama_cpp.vision_model, "model.gguf"),
        },
        ollama: OllamaProviderConfig {
            openai_base_url: setting_or(
                &providers.ollama.openai_base_url,
                "http://localhost:11434/v1",
            ),
            api_key: optional_setting(&providers.ollama.api_key),
            model: ollama_model.clone(),
            model_2: setting_or(&providers.ollama.model_2, &ollama_model),
            model_3: setting_or(&providers.ollama.model_3, &ollama_model),
            vision_model: setting_or(&providers.ollama.vision_model, "llava"),
        },
        gemini: GeminiProviderConfig {
            base_url: "https://generativelanguage.googleapis.com".to_string(),
            api_key: None,
            vision_model: "gemini-2.5-flash".to_string(),
        },
        codex: codex_runtime_config(&providers.codex),
        claude: cli_runtime_config(&providers.claude, claude_cli_args),
        grok: cli_runtime_config(&providers.grok, grok_cli_args),
        copilot: cli_runtime_config(&providers.copilot, copilot_cli_args),
    }
}

fn setting_or(value: &str, default: &str) -> String {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        default.to_string()
    } else {
        trimmed.to_string()
    }
}

fn optional_setting(value: &str) -> Option<String> {
    let trimmed = value.trim();
    (!trimmed.is_empty()).then(|| trimmed.to_string())
}

fn cli_runtime_config(
    settings: &CliSettings,
    args: fn(&CliSettings) -> String,
) -> CliRuntimeConfig {
    CliRuntimeConfig {
        executable: resolved_cli_executable_value(&settings.executable),
        args: split_cli_args(&args(settings)),
        model: optional_setting(&settings.model),
        timeout_seconds: settings.timeout_seconds,
        retries: DEFAULT_CLI_RETRIES,
    }
}

fn codex_runtime_config(settings: &CliSettings) -> CliRuntimeConfig {
    let mut config = cli_runtime_config(settings, codex_cli_args);
    config.model = cli_args_model(&config.args.join(" "));
    config
}

pub fn codex_cli_args(settings: &CliSettings) -> String {
    let mut args = Vec::new();
    let model = settings.model.trim();
    if !model.is_empty() && !cli_args_select_model(&settings.args) {
        args.push("--model".to_string());
        args.push(model.to_string());
    }
    args.push("-c".to_string());
    args.push(format!(
        "model_reasoning_effort={}",
        normalized_reasoning_effort(&settings.reasoning_effort)
    ));
    append_cli_args(args, &settings.args)
}

pub fn claude_cli_args(settings: &CliSettings) -> String {
    append_cli_args(
        vec![
            "--effort".to_string(),
            normalized_reasoning_effort(&settings.reasoning_effort).to_string(),
        ],
        &settings.args,
    )
}

pub fn grok_cli_args(settings: &CliSettings) -> String {
    append_cli_args(
        vec![
            "--no-auto-update".to_string(),
            "--sandbox".to_string(),
            "read-only".to_string(),
            "--disable-web-search".to_string(),
            "--no-subagents".to_string(),
            "--no-plan".to_string(),
            "--always-approve".to_string(),
            "--tools=".to_string(),
        ],
        &settings.args,
    )
}

pub fn copilot_cli_args(settings: &CliSettings) -> String {
    // The Copilot provider bakes the required programmatic flags (`-p`,
    // `--allow-all-tools`, `--no-color`, `-s`) into the vision/summarization
    // crates, so this layer only forwards the user's custom args (for example
    // `--model claude-sonnet-4.5`).
    append_cli_args(Vec::new(), &settings.args)
}

fn append_cli_args(mut args: Vec<String>, custom_args: &str) -> String {
    args.extend(
        custom_args
            .split_whitespace()
            .filter(|arg| !arg.is_empty())
            .map(ToString::to_string),
    );
    args.join(" ")
}

fn split_cli_args(args: &str) -> Vec<String> {
    args.split_whitespace()
        .filter(|arg| !arg.is_empty())
        .map(ToString::to_string)
        .collect()
}

/// Returns true when user-supplied CLI args already select a model. Codex
/// rejects duplicate model selectors, so generated model flags must be
/// omitted whenever a custom selector is present.
pub(crate) fn cli_args_select_model(raw_args: &str) -> bool {
    cli_model_selector(raw_args).is_some()
}

pub(crate) fn cli_args_model(raw_args: &str) -> Option<String> {
    match cli_model_selector(raw_args) {
        Some(CliModelSelector::Value(model)) => Some(model),
        Some(CliModelSelector::Empty) | None => None,
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum CliModelSelector {
    Value(String),
    Empty,
}

fn cli_model_selector(raw_args: &str) -> Option<CliModelSelector> {
    let tokens = split_cli_args(raw_args);
    let mut expect_config_value = false;
    for (index, token) in tokens.iter().enumerate() {
        if expect_config_value {
            expect_config_value = false;
            if let Some(model) = model_config_selector(token) {
                return Some(model);
            }
            continue;
        }

        match token.as_str() {
            "-m" => return tokens.get(index + 1).map(|model| model_selector(model)),
            "--model" => {
                return Some(
                    tokens
                        .get(index + 1)
                        .map_or(CliModelSelector::Empty, |model| model_selector(model)),
                );
            }
            value if value.starts_with("--model=") => {
                return Some(model_selector(value.trim_start_matches("--model=")));
            }
            value if value.starts_with("-m") && value.len() > 2 => {
                return Some(model_selector(
                    value.trim_start_matches("-m").trim_start_matches('='),
                ));
            }
            "-c" | "--config" => expect_config_value = true,
            value if value.starts_with("-c") && value.len() > 2 => {
                if let Some(model) =
                    model_config_selector(value.trim_start_matches("-c").trim_start_matches('='))
                {
                    return Some(model);
                }
            }
            value if value.starts_with("--config=") => {
                if let Some(model) = model_config_selector(value.trim_start_matches("--config=")) {
                    return Some(model);
                }
            }
            _ => {}
        }
    }
    None
}

fn model_config_selector(value: &str) -> Option<CliModelSelector> {
    value
        .trim_start()
        .strip_prefix("model=")
        .map(model_selector)
}

fn model_selector(value: &str) -> CliModelSelector {
    let model = value.trim();
    let model = model
        .strip_prefix('"')
        .and_then(|model| model.strip_suffix('"'))
        .or_else(|| {
            model
                .strip_prefix('\'')
                .and_then(|model| model.strip_suffix('\''))
        })
        .unwrap_or(model)
        .trim();
    if model.is_empty() {
        CliModelSelector::Empty
    } else {
        CliModelSelector::Value(model.to_string())
    }
}

fn normalized_reasoning_effort(value: &str) -> &str {
    match value.trim() {
        "low" => "low",
        "high" => "high",
        "xhigh" => "xhigh",
        "max" => "max",
        _ => "medium",
    }
}

fn fallback_url<'a>(primary: &'a str, fallback: &'a str) -> &'a str {
    if primary.trim().is_empty() {
        fallback
    } else {
        primary
    }
}

pub fn resolved_cli_executable_value(executable: &str) -> String {
    resolve_cli_executable_with_extra_dirs(executable, &default_cli_extra_dirs())
        .map(|path| path.display().to_string())
        .unwrap_or_else(|| executable.trim().to_string())
}

pub fn default_cli_extra_dirs() -> Vec<PathBuf> {
    vec![
        PathBuf::from("/Applications/Codex.app/Contents/Resources"),
        PathBuf::from("/Applications/cmux.app/Contents/Resources/bin"),
    ]
}

#[cfg(test)]
mod tests {
    use super::{
        cli_args_model, cli_args_select_model, codex_cli_args, load_provider_config,
        provider_config_from_settings, split_cli_args, CliSettings, ProviderSettings,
        DEFAULT_CODEX_MODEL,
    };

    #[test]
    fn cli_settings_default_reasoning_effort_to_medium() {
        assert_eq!(CliSettings::new("codex").reasoning_effort, "medium");
        assert_eq!(CliSettings::new("claude").reasoning_effort, "medium");
        assert_eq!(CliSettings::new("grok").reasoning_effort, "medium");
    }

    #[test]
    fn fresh_provider_defaults_select_terra_for_codex_only() {
        let providers = ProviderSettings::default();

        assert_eq!(providers.codex.model, "gpt-5.6-terra");
        assert_eq!(providers.claude.model, "");
        assert_eq!(providers.grok.model, "");
        assert_eq!(providers.copilot.model, "");

        let config = provider_config_from_settings(&providers);
        assert_eq!(
            config.codex.args,
            vec![
                "--model",
                "gpt-5.6-terra",
                "-c",
                "model_reasoning_effort=medium"
            ]
        );
        assert_eq!(
            config
                .codex
                .args
                .iter()
                .filter(|arg| arg.as_str() == "--model")
                .count(),
            1
        );
    }

    #[test]
    fn missing_codex_model_upgrades_to_terra_without_changing_other_cli_defaults() {
        let mut value = serde_json::to_value(ProviderSettings::default()).unwrap();
        for provider in ["codex", "claude", "grok", "copilot"] {
            value[provider].as_object_mut().unwrap().remove("model");
        }

        let providers: ProviderSettings = serde_json::from_value(value).unwrap();

        assert_eq!(providers.codex.model, DEFAULT_CODEX_MODEL);
        assert_eq!(providers.claude.model, "");
        assert_eq!(providers.grok.model, "");
        assert_eq!(providers.copilot.model, "");
        let config = provider_config_from_settings(&providers);
        assert_eq!(
            config.codex.args,
            vec![
                "--model",
                DEFAULT_CODEX_MODEL,
                "-c",
                "model_reasoning_effort=medium"
            ]
        );
        assert_eq!(
            config
                .codex
                .args
                .iter()
                .filter(|arg| arg.as_str() == "--model")
                .count(),
            1
        );
    }

    #[test]
    fn explicit_empty_codex_model_remains_cli_default() {
        let mut value = serde_json::to_value(ProviderSettings::default()).unwrap();
        value["codex"]["model"] = serde_json::json!("");

        let providers: ProviderSettings = serde_json::from_value(value).unwrap();

        assert_eq!(providers.codex.model, "");
        assert_eq!(
            provider_config_from_settings(&providers).codex.args,
            vec!["-c", "model_reasoning_effort=medium"]
        );
    }

    #[test]
    fn codex_cli_args_preserve_legacy_output_when_model_is_empty() {
        let mut settings = CliSettings::new("codex");
        settings.args = "--search".to_string();

        assert_eq!(
            codex_cli_args(&settings),
            "-c model_reasoning_effort=medium --search"
        );
    }

    #[test]
    fn codex_cli_args_emit_one_trimmed_model_before_reasoning_effort() {
        let mut settings = CliSettings::new("codex");
        settings.model = "  gpt-5.6-terra  ".to_string();
        settings.reasoning_effort = "high".to_string();

        assert_eq!(
            codex_cli_args(&settings),
            "--model gpt-5.6-terra -c model_reasoning_effort=high"
        );

        settings.model = "gpt-5.5".to_string();
        assert_eq!(
            codex_cli_args(&settings),
            "--model gpt-5.5 -c model_reasoning_effort=high"
        );

        settings.model = "   ".to_string();
        assert_eq!(codex_cli_args(&settings), "-c model_reasoning_effort=high");
    }

    #[test]
    fn codex_model_override_detector_covers_supported_forms() {
        for args in [
            "-m custom",
            "-mcustom",
            "-m=custom",
            "-m=",
            "--model custom",
            "--model=custom",
            "-c model=custom",
            "-c model=\"custom\"",
            "-cmodel=custom",
            "-c=model=custom",
            "--config model=custom",
            "--config=model=custom",
        ] {
            assert!(cli_args_select_model(args), "did not detect {args:?}");
        }

        for args in [
            "",
            "-m",
            "--search",
            "-c model_reasoning_effort=high",
            "--config=model_reasoning_effort=high",
        ] {
            assert!(
                !cli_args_select_model(args),
                "incorrectly detected {args:?}"
            );
        }
    }

    #[test]
    fn codex_model_override_extractor_covers_supported_forms() {
        for (args, expected) in [
            ("-m custom", Some("custom")),
            ("-mcustom", Some("custom")),
            ("-m=custom", Some("custom")),
            ("--model custom", Some("custom")),
            ("--model=custom", Some("custom")),
            ("-c model=custom", Some("custom")),
            ("-c model=\"custom\"", Some("custom")),
            ("-cmodel=custom", Some("custom")),
            ("-c=model=custom", Some("custom")),
            ("--config model=custom", Some("custom")),
            ("--config=model=custom", Some("custom")),
            ("-m=", None),
            ("-m", None),
        ] {
            assert_eq!(cli_args_model(args).as_deref(), expected, "args={args:?}");
        }
    }

    #[test]
    fn provider_runtime_models_follow_effective_settings_and_args() {
        let mut providers = ProviderSettings::default();
        providers.codex.model = "gpt-codex-configured".to_string();
        providers.claude.model = "claude-configured".to_string();
        providers.grok.model = "grok-configured".to_string();
        providers.copilot.model = "copilot-configured".to_string();

        let config = provider_config_from_settings(&providers);
        assert_eq!(config.codex.model.as_deref(), Some("gpt-codex-configured"));
        assert_eq!(config.claude.model.as_deref(), Some("claude-configured"));
        assert_eq!(config.grok.model.as_deref(), Some("grok-configured"));
        assert_eq!(config.copilot.model.as_deref(), Some("copilot-configured"));

        providers.codex.args = "-mcustom-override".to_string();
        let config = provider_config_from_settings(&providers);
        assert_eq!(config.codex.model.as_deref(), Some("custom-override"));
    }

    #[test]
    fn codex_custom_model_overrides_suppress_generated_model() {
        let mut settings = CliSettings::new("codex");
        settings.model = "gpt-5.6-sol".to_string();

        for args in [
            "-m custom",
            "-mcustom",
            "-m=custom",
            "-m=",
            "--model custom",
            "--model=custom",
            "-c model=custom",
            "-c model=\"custom\"",
            "-cmodel=custom",
            "-c=model=custom",
            "--config model=custom",
            "--config=model=custom",
        ] {
            settings.args = args.to_string();
            let generated = codex_cli_args(&settings);
            assert!(
                !generated.starts_with("--model gpt-5.6-sol"),
                "generated model was not suppressed for {args:?}: {generated}"
            );
        }
    }

    #[test]
    fn codex_cli_args_never_emit_duplicate_model_selectors() {
        for model in ["", "gpt-5.6-sol"] {
            for custom_args in [
                "",
                "--search",
                "-m custom",
                "-mcustom",
                "-m=custom",
                "-m=",
                "--model custom",
                "--model=custom",
                "-c model=custom",
                "--config model=custom",
                "--config=model=custom",
            ] {
                let mut settings = CliSettings::new("codex");
                settings.model = model.to_string();
                settings.args = custom_args.to_string();
                let generated = codex_cli_args(&settings);

                assert!(
                    model_selector_count(&generated) <= 1,
                    "duplicate model selectors for model={model:?}, args={custom_args:?}: {generated}"
                );
            }
        }
    }

    #[test]
    fn cli_settings_without_model_deserialize_to_empty_model() {
        let settings: CliSettings = serde_json::from_value(serde_json::json!({
            "executable": "codex",
            "args": "",
            "reasoning_effort": "medium",
            "timeout_seconds": 600
        }))
        .unwrap();

        assert_eq!(settings.model, "");
    }

    fn model_selector_count(raw_args: &str) -> usize {
        let tokens = split_cli_args(raw_args);
        let mut count = 0;
        let mut expect_config_value = false;
        for (index, token) in tokens.iter().enumerate() {
            if expect_config_value {
                expect_config_value = false;
                if token.starts_with("model=") {
                    count += 1;
                }
                continue;
            }
            match token.as_str() {
                "-m" if tokens.get(index + 1).is_some() => count += 1,
                "--model" => count += 1,
                value if value.starts_with("--model=") => count += 1,
                value
                    if value
                        .strip_prefix("-m")
                        .is_some_and(|model| !model.is_empty()) =>
                {
                    count += 1;
                }
                "-c" | "--config" => expect_config_value = true,
                value
                    if value
                        .strip_prefix("-c")
                        .map(|config| config.trim_start_matches('='))
                        .is_some_and(|config| config.starts_with("model="))
                        || value
                            .strip_prefix("--config=")
                            .is_some_and(|config| config.starts_with("model=")) =>
                {
                    count += 1;
                }
                _ => {}
            }
        }
        count
    }

    #[test]
    fn malformed_settings_file_is_an_error() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("settings.json");
        std::fs::write(&path, "{not json").unwrap();

        let error = load_provider_config(&path).unwrap_err();

        assert!(error.contains("Invalid settings JSON"));
    }

    #[test]
    fn missing_settings_file_returns_none() {
        let temp = tempfile::tempdir().unwrap();

        assert!(load_provider_config(&temp.path().join("missing.json"))
            .unwrap()
            .is_none());
    }

    #[test]
    fn settings_loader_ignores_unknown_desktop_fields() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("settings.json");
        std::fs::write(
            &path,
            r#"{
              "appearance": {"theme": "dark"},
              "providers": {
                "openai": {"base_url": "https://api.openai.com/v1", "api_key": "k", "model": "m1", "model_2": "", "model_3": "", "vision_model": "vm"},
                "llama_cpp": {"base_url": "", "vision_base_url": "", "api_key": "", "model": "", "model_2": "", "model_3": "", "vision_model": ""},
                "ollama": {"openai_base_url": "", "api_key": "", "model": "", "model_2": "", "model_3": "", "vision_model": ""}
              },
              "logging": {"enabled": true}
            }"#,
        )
        .unwrap();

        let config = load_provider_config(&path).unwrap().unwrap();

        assert_eq!(config.openai.model, "m1");
        assert_eq!(config.openai.model_2, "m1");
        assert_eq!(config.openai.model_3, "m1");
        assert_eq!(config.openai.vision_model, "vm");
    }

    #[test]
    fn provider_defaults_map_to_pipeline_config() {
        let providers = Default::default();
        let config = provider_config_from_settings(&providers);

        assert_eq!(config.openai.model, "gpt-4.1-mini");
        assert_eq!(
            config.llama_cpp.vision_base_url,
            "http://localhost:11439/v1"
        );
        assert_eq!(config.ollama.vision_model, "llava");
        assert_eq!(config.codex.timeout_seconds, 600);
    }
}
