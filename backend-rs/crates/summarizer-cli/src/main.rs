use std::{
    collections::BTreeSet,
    env, fs,
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

use clap::Parser;
use serde::Serialize;
use serde_json::Value;
use summarizer_extraction::{probe_path, DocumentProbe};
use summarizer_pipeline::{
    configure_pdfium_library_path,
    settings::{load_provider_config, provider_config_from_settings, ProviderSettings},
    Pipeline, PipelineProgress, PipelineProviderConfig,
};
use summarizer_types::{
    CliProvider, DocumentOutput, PipelineConfig, SummarizerMode, SummarizerProvider, VisionMode,
};
use tokio::{process::Command, time::timeout};

const EXIT_OK: i32 = 0;
const EXIT_FAILURE: i32 = 1;
const EXIT_USAGE: i32 = 2;
const EXIT_ENVIRONMENT: i32 = 3;

#[derive(Debug, Parser)]
#[command(name = "summarizer-cli")]
#[command(version)]
#[command(about = "Run the Document Summarizer pipeline without the desktop app")]
struct Args {
    input: Option<PathBuf>,
    #[arg(long)]
    config_json: Option<String>,
    #[arg(long = "set")]
    set_values: Vec<String>,
    #[arg(long)]
    output: Option<PathBuf>,
    #[arg(long)]
    markdown: bool,
    #[arg(long)]
    settings: Option<PathBuf>,
    #[arg(long)]
    env_providers: bool,
    #[arg(long)]
    pdfium: Option<PathBuf>,
    #[arg(long)]
    job_id: Option<String>,
    #[arg(long)]
    quiet: bool,
    #[arg(long)]
    print_config: bool,
    #[arg(long)]
    doctor: bool,
    #[arg(long)]
    estimate: bool,
}

#[derive(Debug, Serialize)]
struct Check {
    name: String,
    status: &'static str,
    detail: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    remedy: Option<String>,
}

#[derive(Debug, Serialize)]
struct DoctorReport {
    doctor: bool,
    cli_version: &'static str,
    checks: Vec<Check>,
    ok: bool,
}

#[derive(Debug, Serialize)]
struct StagePlan {
    extraction: bool,
    vision: bool,
    summarization: bool,
}

#[derive(Debug, Serialize)]
#[serde(untagged)]
enum CallCount {
    Exact(usize),
    Bound(String),
}

#[derive(Debug, Serialize)]
struct EstimateCalls {
    classify: usize,
    vision_extract: CallCount,
    summarize: usize,
}

#[derive(Debug, Serialize)]
struct BudgetBand {
    max: u64,
    source: &'static str,
}

#[derive(Debug, Serialize)]
struct EstimateReport {
    estimate: bool,
    pages: usize,
    per_page_chars: Vec<usize>,
    per_page_tables: Vec<usize>,
    stages: StagePlan,
    per_stage_calls: EstimateCalls,
    budget_band_seconds: BudgetBand,
    effective_config: Value,
}

#[derive(Debug, Serialize)]
struct ManifestDocument {
    document_id: String,
    filename: String,
    total_pages: usize,
}

#[derive(Debug, Serialize)]
struct Manifest {
    job_id: String,
    status: &'static str,
    input: String,
    output_json_path: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    output_md_path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    document: Option<ManifestDocument>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
    duration_ms: u128,
}

#[tokio::main]
async fn main() {
    let args = Args::parse();
    std::process::exit(run(args).await);
}

async fn run(args: Args) -> i32 {
    let started = Instant::now();
    let job_id = args
        .job_id
        .clone()
        .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());

    let config = match build_config(&args) {
        Ok(config) => config,
        Err(error) => {
            eprintln!("{error}");
            return EXIT_USAGE;
        }
    };

    if args.print_config {
        println!(
            "{}",
            serde_json::to_string(&config).expect("config serializes")
        );
        return EXIT_OK;
    }

    if args.doctor {
        return run_doctor(&args, &config).await;
    }

    let Some(input) = args.input.clone().map(ExpandHome::expand_home) else {
        eprintln!("Input file is required unless --doctor or --print-config is used.");
        return EXIT_USAGE;
    };
    let output_json_path = args
        .output
        .clone()
        .unwrap_or_else(|| default_output_path(&input));
    let output_md_path = args.markdown.then(|| output_json_path.with_extension("md"));

    if !input.is_file() {
        print_manifest(Manifest::failed(
            job_id,
            &input,
            &output_json_path,
            output_md_path.as_deref(),
            format!("Input file not found: {}", input.display()),
            started.elapsed().as_millis(),
        ));
        return EXIT_ENVIRONMENT;
    }

    if args.estimate {
        return run_estimate(&args, &input, &config);
    }

    let provider_config = match provider_config(&args) {
        Ok(config) => config,
        Err(error) => {
            print_manifest(Manifest::failed(
                job_id,
                &input,
                &output_json_path,
                output_md_path.as_deref(),
                error,
                started.elapsed().as_millis(),
            ));
            return EXIT_ENVIRONMENT;
        }
    };

    if requires_pdfium(&input) {
        match resolve_pdfium_path(args.pdfium.as_deref()) {
            Ok(path) => {
                if let Err(error) = configure_pdfium_library_path(path) {
                    print_manifest(Manifest::failed(
                        job_id,
                        &input,
                        &output_json_path,
                        output_md_path.as_deref(),
                        error.to_string(),
                        started.elapsed().as_millis(),
                    ));
                    return EXIT_ENVIRONMENT;
                }
            }
            Err(error) => {
                print_manifest(Manifest::failed(
                    job_id,
                    &input,
                    &output_json_path,
                    output_md_path.as_deref(),
                    error,
                    started.elapsed().as_millis(),
                ));
                return EXIT_ENVIRONMENT;
            }
        }
    }

    let quiet = args.quiet;
    let pipeline = Pipeline::with_provider_config(provider_config);
    let result = pipeline
        .run_path_with_progress(&job_id, &input, &config, move |progress| {
            if !quiet {
                eprintln!("{}", format_progress(&progress));
            }
        })
        .await;

    match result {
        Ok(output) => match write_outputs(&output, &output_json_path, output_md_path.as_deref()) {
            Ok(()) => {
                print_manifest(Manifest::completed(
                    job_id,
                    &input,
                    &output_json_path,
                    output_md_path.as_deref(),
                    &output,
                    started.elapsed().as_millis(),
                ));
                EXIT_OK
            }
            Err(error) => {
                print_manifest(Manifest::failed(
                    job_id,
                    &input,
                    &output_json_path,
                    output_md_path.as_deref(),
                    error,
                    started.elapsed().as_millis(),
                ));
                EXIT_FAILURE
            }
        },
        Err(error) => {
            print_manifest(Manifest::failed(
                job_id,
                &input,
                &output_json_path,
                output_md_path.as_deref(),
                error.to_string(),
                started.elapsed().as_millis(),
            ));
            EXIT_FAILURE
        }
    }
}

fn build_config(args: &Args) -> Result<PipelineConfig, String> {
    let raw = args.config_json.as_deref().unwrap_or("{}");
    let mut value = serde_json::to_value(PipelineConfig::merge_json_onto_desktop_default(raw)?)
        .map_err(|err| format!("Could not encode effective config: {err}"))?;
    let object = value
        .as_object_mut()
        .ok_or_else(|| "Could not encode effective config as an object".to_string())?;
    for assignment in &args.set_values {
        let (key, raw_value) = assignment
            .split_once('=')
            .ok_or_else(|| format!("--set must be key=value, got '{assignment}'"))?;
        if key.trim().is_empty() {
            return Err("--set key must not be empty".to_string());
        }
        let parsed = serde_json::from_str::<Value>(raw_value)
            .unwrap_or_else(|_| Value::String(raw_value.to_string()));
        object.insert(key.trim().to_string(), parsed);
    }
    serde_json::from_value(value).map_err(|err| format!("Invalid config JSON: {err}"))
}

fn provider_config(args: &Args) -> Result<PipelineProviderConfig, String> {
    if args.env_providers {
        return Ok(PipelineProviderConfig::from_env());
    }
    let path = args
        .settings
        .clone()
        .unwrap_or_else(default_settings_path)
        .expand_home();
    provider_config_from_path(&path)
}

fn provider_config_from_path(path: &Path) -> Result<PipelineProviderConfig, String> {
    load_provider_config(path).map(|config| {
        config.unwrap_or_else(|| provider_config_from_settings(&ProviderSettings::default()))
    })
}

async fn run_doctor(args: &Args, config: &PipelineConfig) -> i32 {
    let mut checks = Vec::new();
    let provider_config = doctor_settings_check(args, &mut checks);
    doctor_pdfium_check(args, &mut checks);
    doctor_provider_checks(config, &provider_config, &mut checks).await;
    let ok = checks.iter().all(|check| check.status != "fail");
    println!(
        "{}",
        serde_json::to_string(&DoctorReport {
            doctor: true,
            cli_version: env!("CARGO_PKG_VERSION"),
            checks,
            ok,
        })
        .expect("doctor report serializes")
    );
    if ok {
        EXIT_OK
    } else {
        EXIT_FAILURE
    }
}

fn doctor_settings_check(args: &Args, checks: &mut Vec<Check>) -> PipelineProviderConfig {
    if args.env_providers {
        checks.push(Check::skip(
            "settings",
            "--env-providers set; using environment provider values.",
        ));
        return PipelineProviderConfig::from_env();
    }

    let path = args
        .settings
        .clone()
        .unwrap_or_else(default_settings_path)
        .expand_home();
    match load_provider_config(&path) {
        Ok(Some(config)) => {
            checks.push(Check::ok(
                "settings",
                format!("Loaded provider settings from {}.", path.display()),
            ));
            config
        }
        Ok(None) => {
            checks.push(Check::ok(
                "settings",
                format!(
                    "No settings file at {}; using default provider settings.",
                    path.display()
                ),
            ));
            provider_config_from_settings(&ProviderSettings::default())
        }
        Err(error) => {
            checks.push(Check::fail(
                "settings",
                error,
                "Fix the settings JSON or pass --env-providers.",
            ));
            PipelineProviderConfig::from_env()
        }
    }
}

fn doctor_pdfium_check(args: &Args, checks: &mut Vec<Check>) {
    let Some(input) = args.input.as_ref().map(|input| input.clone().expand_home()) else {
        checks.push(Check::skip("pdfium", "No input file was provided."));
        return;
    };
    if !requires_pdfium(&input) {
        checks.push(Check::skip(
            "pdfium",
            format!("{} does not require PDFium.", input.display()),
        ));
        return;
    }
    match resolve_pdfium_path(args.pdfium.as_deref()) {
        Ok(path) => checks.push(Check::ok(
            "pdfium",
            format!("Resolved PDFium library at {}.", path.display()),
        )),
        Err(error) => checks.push(Check::fail(
            "pdfium",
            error,
            "Set --pdfium or SUMMARIZER_PDFIUM to the bundled PDFium library.",
        )),
    }
}

async fn doctor_provider_checks(
    config: &PipelineConfig,
    provider_config: &PipelineProviderConfig,
    checks: &mut Vec<Check>,
) {
    for provider in required_providers(config) {
        match provider.as_str() {
            "llama_cpp" => {
                checks.push(
                    http_models_check(
                        "provider:llama_cpp",
                        &provider_config.llama_cpp.base_url,
                        "Start llama.cpp or fix providers.llama_cpp.base_url.",
                    )
                    .await,
                );
            }
            "llama_cpp_vision" => {
                checks.push(
                    http_models_check(
                        "provider:llama_cpp_vision",
                        &provider_config.llama_cpp.vision_base_url,
                        "Start the llama.cpp vision server or fix providers.llama_cpp.vision_base_url.",
                    )
                    .await,
                );
            }
            "ollama" => {
                checks.push(
                    http_models_check(
                        "provider:ollama",
                        &provider_config.ollama.openai_base_url,
                        "Start Ollama's OpenAI-compatible endpoint or fix providers.ollama.openai_base_url.",
                    )
                    .await,
                );
            }
            "openai" => {
                if provider_config.openai.api_key.is_some() {
                    checks.push(Check::ok(
                        "provider:openai",
                        format!(
                            "OpenAI API key is present; base_url={}.",
                            provider_config.openai.base_url
                        ),
                    ));
                } else {
                    checks.push(Check::fail(
                        "provider:openai",
                        "OpenAI API key is empty.",
                        "Set providers.openai.api_key in settings.json or OPENAI_API_KEY with --env-providers.",
                    ));
                }
            }
            "codex" => checks
                .push(cli_version_check("provider:codex", &provider_config.codex.executable).await),
            "claude" => checks.push(
                cli_version_check("provider:claude", &provider_config.claude.executable).await,
            ),
            "grok" => checks
                .push(cli_version_check("provider:grok", &provider_config.grok.executable).await),
            "copilot" => checks.push(
                cli_version_check("provider:copilot", &provider_config.copilot.executable).await,
            ),
            "gemini" => {
                if provider_config.gemini.api_key.is_some() {
                    checks.push(Check::ok(
                        "provider:gemini",
                        format!(
                            "Gemini API key is present; base_url={}.",
                            provider_config.gemini.base_url
                        ),
                    ));
                } else {
                    checks.push(Check::fail(
                        "provider:gemini",
                        "Gemini API key is empty.",
                        "Set GEMINI_API_KEY with --env-providers.",
                    ));
                }
            }
            "deepseek" => checks.push(Check::fail(
                "provider:deepseek",
                "DeepSeek vision mode is not implemented in the Rust backend.",
                "Choose another vision provider.",
            )),
            _ => {}
        }
    }
}

fn required_providers(config: &PipelineConfig) -> BTreeSet<String> {
    let mut providers = BTreeSet::new();
    let vision_active =
        config.vision_mode != VisionMode::None && !config.extract_only && !config.text_only;
    if vision_active {
        if !config.vision_skip_classification {
            add_vision_provider(
                resolve_cli_vision_mode(
                    config.vision_classifier_mode.unwrap_or(config.vision_mode),
                    config.vision_cli_provider,
                ),
                &mut providers,
            );
        }
        add_vision_provider(
            resolve_cli_vision_mode(
                config.vision_extractor_mode.unwrap_or(config.vision_mode),
                config.vision_cli_provider,
            ),
            &mut providers,
        );
    }
    if config.run_summarization
        && config.summarizer_mode != SummarizerMode::Skip
        && !config.extract_only
    {
        add_summarizer_provider(resolve_cli_summarizer_provider(config), &mut providers);
    }
    providers
}

fn add_vision_provider(mode: VisionMode, providers: &mut BTreeSet<String>) {
    providers.insert(
        match mode {
            VisionMode::None => return,
            VisionMode::Deepseek => "deepseek",
            VisionMode::Gemini => "gemini",
            VisionMode::Openai => "openai",
            VisionMode::Ollama => "ollama",
            VisionMode::LlamaCpp => "llama_cpp_vision",
            VisionMode::Codex => "codex",
            VisionMode::Claude => "claude",
            VisionMode::Grok => "grok",
            VisionMode::Copilot => "copilot",
        }
        .to_string(),
    );
}

fn add_summarizer_provider(provider: SummarizerProvider, providers: &mut BTreeSet<String>) {
    providers.insert(
        match provider {
            SummarizerProvider::Ollama => "ollama",
            SummarizerProvider::LlamaCpp => "llama_cpp",
            SummarizerProvider::Openai => "openai",
            SummarizerProvider::Codex => "codex",
            SummarizerProvider::Claude => "claude",
            SummarizerProvider::Grok => "grok",
            SummarizerProvider::Copilot => "copilot",
        }
        .to_string(),
    );
}

fn resolve_cli_vision_mode(mode: VisionMode, cli_provider: Option<CliProvider>) -> VisionMode {
    match (mode, cli_provider) {
        (
            VisionMode::Codex | VisionMode::Claude | VisionMode::Grok | VisionMode::Copilot,
            Some(CliProvider::Codex),
        ) => VisionMode::Codex,
        (
            VisionMode::Codex | VisionMode::Claude | VisionMode::Grok | VisionMode::Copilot,
            Some(CliProvider::Claude),
        ) => VisionMode::Claude,
        (
            VisionMode::Codex | VisionMode::Claude | VisionMode::Grok | VisionMode::Copilot,
            Some(CliProvider::Grok),
        ) => VisionMode::Grok,
        (
            VisionMode::Codex | VisionMode::Claude | VisionMode::Grok | VisionMode::Copilot,
            Some(CliProvider::Copilot),
        ) => VisionMode::Copilot,
        _ => mode,
    }
}

fn resolve_cli_summarizer_provider(config: &PipelineConfig) -> SummarizerProvider {
    match (config.summarizer_provider, config.summarizer_cli_provider) {
        (
            SummarizerProvider::Codex
            | SummarizerProvider::Claude
            | SummarizerProvider::Grok
            | SummarizerProvider::Copilot,
            Some(CliProvider::Codex),
        ) => SummarizerProvider::Codex,
        (
            SummarizerProvider::Codex
            | SummarizerProvider::Claude
            | SummarizerProvider::Grok
            | SummarizerProvider::Copilot,
            Some(CliProvider::Claude),
        ) => SummarizerProvider::Claude,
        (
            SummarizerProvider::Codex
            | SummarizerProvider::Claude
            | SummarizerProvider::Grok
            | SummarizerProvider::Copilot,
            Some(CliProvider::Grok),
        ) => SummarizerProvider::Grok,
        (
            SummarizerProvider::Codex
            | SummarizerProvider::Claude
            | SummarizerProvider::Grok
            | SummarizerProvider::Copilot,
            Some(CliProvider::Copilot),
        ) => SummarizerProvider::Copilot,
        _ => config.summarizer_provider,
    }
}

async fn http_models_check(name: &str, base_url: &str, remedy: &str) -> Check {
    let url = format!("{}/models", base_url.trim_end_matches('/'));
    let client = match reqwest::Client::builder()
        .timeout(Duration::from_secs(5))
        .build()
    {
        Ok(client) => client,
        Err(error) => {
            return Check::fail(
                name,
                format!("Could not build HTTP client: {error}"),
                remedy,
            );
        }
    };
    match client.get(&url).send().await {
        Ok(response) if response.status().is_success() => {
            Check::ok(name, format!("GET {url} returned {}.", response.status()))
        }
        Ok(response) => Check::fail(
            name,
            format!("GET {url} returned {}.", response.status()),
            remedy,
        ),
        Err(error) => Check::fail(name, format!("GET {url} failed: {error}"), remedy),
    }
}

async fn cli_version_check(name: &str, executable: &str) -> Check {
    let result = timeout(
        Duration::from_secs(5),
        Command::new(executable).arg("--version").output(),
    )
    .await;
    match result {
        Ok(Ok(output)) if output.status.success() => {
            Check::ok(name, format!("{executable} --version exited successfully."))
        }
        Ok(Ok(output)) => Check::fail(
            name,
            format!("{executable} --version exited with {}.", output.status),
            "Install the provider CLI or fix the executable path in settings.",
        ),
        Ok(Err(error)) => Check::fail(
            name,
            format!("Could not run {executable} --version: {error}"),
            "Install the provider CLI or fix the executable path in settings.",
        ),
        Err(_) => Check::fail(
            name,
            format!("{executable} --version timed out after 5 seconds."),
            "Fix the provider CLI executable or its startup environment.",
        ),
    }
}

fn run_estimate(args: &Args, input: &Path, config: &PipelineConfig) -> i32 {
    if requires_pdfium(input) {
        match resolve_pdfium_path(args.pdfium.as_deref()) {
            Ok(path) => {
                if let Err(error) = configure_pdfium_library_path(path) {
                    eprintln!("{error}");
                    return EXIT_ENVIRONMENT;
                }
            }
            Err(error) => {
                eprintln!("{error}");
                return EXIT_ENVIRONMENT;
            }
        }
    }

    let probe = match probe_path(input, config) {
        Ok(probe) => probe,
        Err(error) => {
            eprintln!("{error}");
            return EXIT_FAILURE;
        }
    };
    let report = estimate_report(&probe, config);
    println!(
        "{}",
        serde_json::to_string(&report).expect("estimate report serializes")
    );
    EXIT_OK
}

fn estimate_report(probe: &DocumentProbe, config: &PipelineConfig) -> EstimateReport {
    let vision =
        config.vision_mode != VisionMode::None && !config.extract_only && !config.text_only;
    let summarization = config.run_summarization
        && config.summarizer_mode != SummarizerMode::Skip
        && !config.extract_only;
    let classify = if vision && !config.vision_skip_classification {
        probe.pages
    } else {
        0
    };
    let vision_extract = if !vision {
        CallCount::Exact(0)
    } else if config.vision_skip_classification {
        CallCount::Exact(probe.pages)
    } else {
        CallCount::Bound(format!("<= {} (classifier-dependent)", probe.pages))
    };
    let summarize_per_page = if summarization {
        let detailed = if config.summarizer_detailed_extraction {
            3
        } else {
            1
        };
        let insight = usize::from(
            config.summarizer_insight_mode && config.summarizer_mode == SummarizerMode::Full,
        );
        detailed + insight
    } else {
        0
    };

    EstimateReport {
        estimate: true,
        pages: probe.pages,
        per_page_chars: probe.per_page_chars.clone(),
        per_page_tables: probe.per_page_tables.clone(),
        stages: StagePlan {
            extraction: true,
            vision,
            summarization,
        },
        per_stage_calls: EstimateCalls {
            classify,
            vision_extract,
            summarize: probe.pages * summarize_per_page,
        },
        budget_band_seconds: BudgetBand {
            max: probe.pages as u64 * config.max_seconds_per_page,
            source: "budget-derived",
        },
        effective_config: serde_json::to_value(config).expect("config serializes"),
    }
}

fn write_outputs(
    output: &DocumentOutput,
    output_json_path: &Path,
    output_md_path: Option<&Path>,
) -> Result<(), String> {
    if let Some(parent) = output_json_path.parent() {
        fs::create_dir_all(parent).map_err(|err| {
            format!(
                "Could not create output directory {}: {err}",
                parent.display()
            )
        })?;
    }
    let json = serde_json::to_vec_pretty(output)
        .map_err(|err| format!("Could not serialize output JSON: {err}"))?;
    fs::write(output_json_path, json)
        .map_err(|err| format!("Could not write {}: {err}", output_json_path.display()))?;

    if let Some(path) = output_md_path {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(|err| {
                format!(
                    "Could not create markdown directory {}: {err}",
                    parent.display()
                )
            })?;
        }
        fs::write(path, output.to_markdown())
            .map_err(|err| format!("Could not write {}: {err}", path.display()))?;
    }
    Ok(())
}

fn default_output_path(input: &Path) -> PathBuf {
    let parent = input.parent().unwrap_or_else(|| Path::new("."));
    let stem = input
        .file_stem()
        .and_then(|value| value.to_str())
        .filter(|value| !value.is_empty())
        .unwrap_or("output");
    parent.join(format!("{stem}_output.json"))
}

fn default_settings_path() -> PathBuf {
    let home = env::var_os("SUMMARIZER_HOME")
        .or_else(|| env::var_os("HOME"))
        .or_else(|| env::var_os("USERPROFILE"))
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."));
    home.join(".summarizer").join("settings.json")
}

fn requires_pdfium(input: &Path) -> bool {
    matches!(
        input
            .extension()
            .and_then(|value| value.to_str())
            .map(|value| value.to_ascii_lowercase())
            .as_deref(),
        Some("pdf" | "pptx")
    )
}

fn resolve_pdfium_path(explicit: Option<&Path>) -> Result<PathBuf, String> {
    let mut tried = Vec::new();
    if let Some(path) = explicit {
        tried.push(path.to_path_buf());
        return if path.is_file() {
            Ok(path.to_path_buf())
        } else {
            Err(pdfium_resolution_error(&tried))
        };
    }
    if let Some(path) = env::var_os("SUMMARIZER_PDFIUM").map(PathBuf::from) {
        tried.push(path.clone());
        if path.is_file() {
            return Ok(path);
        }
    }

    for path in installed_pdfium_candidates()
        .into_iter()
        .chain(dev_pdfium_candidates())
    {
        tried.push(path.clone());
        if path.is_file() {
            return Ok(path);
        }
    }

    Err(pdfium_resolution_error(&tried))
}

fn pdfium_resolution_error(tried: &[PathBuf]) -> String {
    let tried = tried
        .iter()
        .map(|path| path.display().to_string())
        .collect::<Vec<_>>()
        .join(", ");
    format!("Could not resolve PDFium library. Tried: {tried}")
}

fn installed_pdfium_candidates() -> Vec<PathBuf> {
    let library = pdfium_library_name();
    let mut candidates = Vec::new();
    #[cfg(target_os = "macos")]
    candidates.push(
        PathBuf::from("/Applications/Document Summarizer.app/Contents/Resources/resources/pdfium")
            .join(library),
    );
    if let Ok(exe) = env::current_exe() {
        if let Some(dir) = exe.parent() {
            candidates.push(dir.join("resources").join("pdfium").join(library));
            candidates.push(
                dir.join("..")
                    .join("Resources")
                    .join("resources")
                    .join("pdfium")
                    .join(library),
            );
            candidates.push(dir.join("..").join("pdfium").join(library));
        }
    }
    candidates
}

fn dev_pdfium_candidates() -> Vec<PathBuf> {
    vec![PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../../apps/desktop/src-tauri/resources/pdfium")
        .join(pdfium_library_name())]
}

fn pdfium_library_name() -> &'static str {
    #[cfg(target_os = "windows")]
    {
        "pdfium.dll"
    }
    #[cfg(target_os = "macos")]
    {
        "libpdfium.dylib"
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        "libpdfium.so"
    }
}

fn format_progress(progress: &PipelineProgress) -> String {
    let page = match (progress.page_number, progress.total_pages) {
        (Some(page), Some(total)) => format!(" {page}/{total}"),
        _ => String::new(),
    };
    format!(
        "[{}/{}] {}{} {}% {}",
        progress.stage_index,
        progress.total_stages,
        progress.stage.as_str(),
        page,
        progress.progress,
        progress.message
    )
}

impl Manifest {
    fn completed(
        job_id: String,
        input: &Path,
        output_json_path: &Path,
        output_md_path: Option<&Path>,
        output: &DocumentOutput,
        duration_ms: u128,
    ) -> Self {
        Self {
            job_id,
            status: "completed",
            input: input.display().to_string(),
            output_json_path: output_json_path.display().to_string(),
            output_md_path: output_md_path.map(|path| path.display().to_string()),
            document: Some(ManifestDocument {
                document_id: output.document.document_id.clone(),
                filename: output.document.filename.clone(),
                total_pages: output.document.total_pages,
            }),
            error: None,
            duration_ms,
        }
    }

    fn failed(
        job_id: String,
        input: &Path,
        output_json_path: &Path,
        output_md_path: Option<&Path>,
        error: String,
        duration_ms: u128,
    ) -> Self {
        Self {
            job_id,
            status: "failed",
            input: input.display().to_string(),
            output_json_path: output_json_path.display().to_string(),
            output_md_path: output_md_path.map(|path| path.display().to_string()),
            document: None,
            error: Some(error),
            duration_ms,
        }
    }
}

impl Check {
    fn ok(name: impl Into<String>, detail: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            status: "ok",
            detail: detail.into(),
            remedy: None,
        }
    }

    fn skip(name: impl Into<String>, detail: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            status: "skip",
            detail: detail.into(),
            remedy: None,
        }
    }

    fn fail(name: impl Into<String>, detail: impl Into<String>, remedy: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            status: "fail",
            detail: detail.into(),
            remedy: Some(remedy.into()),
        }
    }
}

fn print_manifest(manifest: Manifest) {
    println!(
        "{}",
        serde_json::to_string(&manifest).expect("manifest serializes")
    );
}

trait ExpandHome {
    fn expand_home(self) -> PathBuf;
}

impl ExpandHome for PathBuf {
    fn expand_home(self) -> PathBuf {
        let Some(text) = self.to_str() else {
            return self;
        };
        if text == "~" || text.starts_with("~/") {
            if let Some(home) = env::var_os("HOME").or_else(|| env::var_os("USERPROFILE")) {
                let suffix = text.trim_start_matches('~').trim_start_matches('/');
                return PathBuf::from(home).join(suffix);
            }
        }
        self
    }
}

impl ExpandHome for &Path {
    fn expand_home(self) -> PathBuf {
        self.to_path_buf().expand_home()
    }
}

#[cfg(test)]
mod tests {
    use super::provider_config_from_path;

    #[test]
    fn missing_settings_use_fresh_codex_model_default() {
        let temp = tempfile::tempdir().unwrap();
        let config = provider_config_from_path(&temp.path().join("missing-settings.json")).unwrap();

        assert_eq!(config.codex.model.as_deref(), Some("gpt-5.6-terra"));
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
}
