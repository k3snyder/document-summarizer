# Document Summarizer

> Turn PDFs, PPTX, DOCX, and Markdown into structured, AI-ready data. Combines document extraction, vision, and summarization in one pipeline to capture the full context of every page - text, tables, and diagrams - ready for AI.

[![Release](https://img.shields.io/github/v/release/k3snyder/document-summarizer?sort=semver)](https://github.com/k3snyder/document-summarizer/releases)
[![License: MIT](https://img.shields.io/badge/License-MIT-yellow.svg)](LICENSE)
[![Platform: macOS Apple Silicon](https://img.shields.io/badge/platform-macOS%20Apple%20Silicon-black?logo=apple)](#download)
[![CI](https://github.com/k3snyder/document-summarizer/actions/workflows/ci.yml/badge.svg)](https://github.com/k3snyder/document-summarizer/actions/workflows/ci.yml)
[![Built with Rust](https://img.shields.io/badge/Rust-1.88+-orange?logo=rust)](https://www.rust-lang.org/)
[![Tauri 2](https://img.shields.io/badge/Tauri-2-24C8DB?logo=tauri)](https://tauri.app/)

<!-- Replace with a real 5-10s screen recording: drag a PDF in, watch structured JSON + Markdown appear. -->
![Document Summarizer demo](docs/assets/demo.gif)

A **Rust + Tauri desktop app** that combines three stages - extraction, vision, and summarization - into one pipeline. Most extractors only pull raw text and lose everything visual; Document Summarizer captures the **full context of every page** (text, tables, and diagrams) and turns it into structured, AI-ready data.

## Why Document Summarizer

- 🧠 **Full-context pipeline** - extraction + vision + summarization working together, so nothing on the page is lost on the way to your model.
- 👁️ **Vision-aware** - classifies and extracts diagrams, screenshots, photos, and meaningful tables, not just raw text.
- 📄 **Multi-format** - PDF, PPTX, DOCX, TXT, and Markdown in one pipeline.
- 🧩 **Structured output** - a unified JSON schema (plus Markdown) for every document, ready for RAG or downstream AI.
- 🔌 **Bring your own model** - llama.cpp, Ollama, OpenAI, Codex CLI, Claude CLI, or Grok CLI. Run fully offline with a local model, or use a cloud provider.
- ✅ **Quality-validated** - multi-attempt summarization with relevancy thresholds and per-page budgets.

## Table of Contents

- [Download](#download)
- [Quick Start (from source)](#quick-start-from-source)
- [How It Works](#how-it-works)
- [Example Output](#example-output)
- [Requirements](#requirements)
- [Configuration](#configuration)
- [Usage Details](#usage-details)
- [Development](#development)
- [Project Structure](#project-structure)
- [Tech Stack](#tech-stack)
- [Status & Scope](#status--scope)
- [License](#license)

## Download

Grab the latest macOS (Apple Silicon) build from the **[Releases page](https://github.com/k3snyder/document-summarizer/releases)**.

A Windows build exists as an experimental, unsigned validation lane - see [Status & Scope](#status--scope) before relying on it.

## Quick Start (from source)

```bash
git clone https://github.com/k3snyder/document-summarizer.git
cd document-summarizer/apps/desktop
npm install
npm run tauri:dev
```

The desktop app runs the `summarizer-pipeline` in-process through Tauri commands. It does not start an HTTP server or bind a port.

You'll also want a local LLM running (llama.cpp recommended) - see [Recommended Local Topology](#recommended-local-topology).

## How It Works

```
STAGE 1: EXTRACTION → STAGE 2: VISION → STAGE 3: SUMMARIZATION
   PDF/PPTX/DOCX/TXT     Classify & Extract      Notes & Topics
```

The pipeline produces a unified JSON schema at each stage:

```json
{
  "document": { "document_id", "filename", "total_pages", "metadata" },
  "pages": [
    {
      "chunk_id", "text", "tables",
      "image_base64", "image_text", "image_classifier",
      "summary_notes", "summary_topics", "summary_relevancy"
    }
  ]
}
```

Each job writes both `output.json` and `output.md` under `~/.summarizer/jobs/<job_id>/`.

## Example Output

Feed in a slide deck or PDF, and each page/chunk comes back structured:

```json
{
  "document": { "filename": "q3-strategy.pdf", "total_pages": 12 },
  "pages": [
    {
      "chunk_id": "p3",
      "text": "Q3 revenue grew 18% QoQ ...",
      "tables": [["Region", "Rev"], ["NA", "$4.2M"]],
      "image_classifier": "YES",
      "image_text": "Bar chart: revenue by region, NA leading",
      "summary_notes": "Revenue up 18% QoQ, driven by NA region ...",
      "summary_topics": ["revenue", "regional performance"],
      "summary_relevancy": 0.91
    }
  ]
}
```

The same content is also rendered to readable Markdown for humans.

## Requirements

**Core:**

- Rust 1.88+
- Node.js 20+
- A local LLM: **llama.cpp** (recommended default) or **Ollama**

**Bundled / automatic:**

- PDFium (bundled in the desktop app; see `apps/desktop/src-tauri/resources/pdfium/PDFIUM_VERSION.txt`)

**Optional:**

- LibreOffice / `soffice` - only for PPTX jobs that use slide screenshots ([install guide](#libreoffice-for-pptx-vision))
- OpenAI, Codex CLI, Claude CLI, or Grok CLI - alternative providers

<details>
<summary><strong>LibreOffice for PPTX vision</strong> (only needed for slide-screenshot vision)</summary>

LibreOffice is required only for PPTX jobs that use slide screenshots for vision processing. It is intentionally **not** bundled because it is a large external dependency.

Install on macOS with Homebrew:

```bash
brew install --cask libreoffice
```

Or download the macOS package from the official LibreOffice download page and install it into `/Applications`.

On Windows, install with `winget`:

```powershell
winget install --id TheDocumentFoundation.LibreOffice --exact --source winget --accept-package-agreements --accept-source-agreements
```

If installation needs machine-wide elevation, open PowerShell as Administrator and run:

```powershell
winget install --id TheDocumentFoundation.LibreOffice --exact --source winget --scope machine --accept-package-agreements --accept-source-agreements
```

The `winget` agreement flags must be typed as uninterrupted arguments (use `--accept-package-agreements`, not `--accept- package-agreements`).

If `winget` is unavailable, download the Windows installer from the official LibreOffice download page and install with default options. The app searches `PATH`, `SOFFICE_BIN`, `LIBREOFFICE_BIN`, and the default `C:\Program Files\LibreOffice\program\soffice.exe` location.

Verify the desktop app can find `soffice`:

```bash
/Applications/LibreOffice.app/Contents/MacOS/soffice --version
```

```powershell
& "C:\Program Files\LibreOffice\program\soffice.exe" --version
```

After installing on Windows, fully quit and reopen the desktop app before retrying the PPTX job. If the app still cannot find LibreOffice, set an explicit user environment variable and restart:

```powershell
setx SOFFICE_BIN "C:\Program Files\LibreOffice\program\soffice.exe"
```

For a temporary PowerShell-only dev session:

```powershell
$env:SOFFICE_BIN = "C:\Program Files\LibreOffice\program\soffice.exe"
```

For macOS/Linux development or custom installs, point the app at an explicit binary:

```bash
export SOFFICE_BIN="/Applications/LibreOffice.app/Contents/MacOS/soffice"
```

To process a PPTX **without** slide screenshots, enable Advanced Mode and turn on Skip Slide Screenshots. This bypasses LibreOffice rendering but removes slide screenshot images from vision processing.

</details>

<details>
<summary><strong>Optional CLI tools</strong> (Codex / Claude)</summary>

```bash
# Codex CLI
npm install -g @openai/codex

# Claude CLI
npm install -g @anthropic-ai/claude-code
```

Configure Grok, Codex, and Claude executable paths in Desktop Settings.

</details>

## Configuration

### Recommended Local Topology

- `llama.cpp` on `11440` for primary text + summarization
- `llama.cpp` on `11439` for multimodal vision
- `Ollama` as an optional fallback local provider

Save the same URLs in Desktop Settings, then verify:

```bash
curl http://localhost:11440/v1/models
curl http://localhost:11440/health
curl http://localhost:11439/v1/models
curl http://localhost:11439/health
```

### Pipeline Configuration

```json
{
  "run_extraction": true,
  "extract_only": false,
  "skip_tables": false,
  "skip_images": false,
  "skip_pptx_tables": false,
  "text_only": false,
  "pdf_image_dpi": 200,
  "vision_mode": "llama_cpp",
  "vision_classifier_mode": null,
  "vision_extractor_mode": null,
  "vision_cli_provider": null,
  "vision_skip_classification": false,
  "chunk_size": 3000,
  "chunk_overlap": 80,
  "run_summarization": true,
  "summarizer_mode": "full",
  "summarizer_provider": "llama_cpp",
  "summarizer_cli_provider": null,
  "summarizer_detailed_extraction": false,
  "summarizer_insight_mode": false,
  "max_tokens_per_page": 100000,
  "max_seconds_per_page": 300,
  "keep_base64_images": false
}
```

- **Vision Modes**: `none`, `llama_cpp`, `ollama`, `openai`, `codex`, `claude`, `grok`. REST/server workflows additionally accept `gemini`; the desktop app blocks Gemini jobs.
- **Vision CLI Providers**: `codex`, `claude`, `grok`
- **Summarizer Modes**: `full` (notes + topics), `topics-only`, `skip`
- **Summarizer Providers**: `llama_cpp`, `ollama`, `openai`, `codex`, `claude`, `grok`
- **Summarizer CLI Providers**: `codex`, `claude`, `grok`
- **Summarizer Detailed Extraction**: when enabled, runs summarization extraction multiple times and synthesizes notes for comprehensive coverage.
- **Page Budgets**: `max_tokens_per_page` and `max_seconds_per_page` cap per-page summarization work. When a budget is exhausted, the pipeline returns best-effort output and records `summary_budget_exhausted`.
- **Skip Classification**: set `vision_skip_classification` to `true` to skip the visual classifier and run vision extraction directly for every page/slide/chunk with an image payload.
- **Vision Classification Policy**: the classifier is conservative about page furniture. Footer/header logos, page numbers, copyright lines, decorative backgrounds, branding accents, and TOC leader lines do not trigger extraction on their own. Pages are classified `YES` when they contain substantive visual information that would be lost with text-only extraction.

### Desktop Settings

The desktop app stores provider configuration in `~/.summarizer/settings.json` (macOS/Linux) or `%USERPROFILE%\.summarizer\settings.json` (Windows), then passes those values into each embedded pipeline run. The headless server and CLI use environment variables for provider configuration.

Supported provider settings:

- llama.cpp base URL, vision base URL, API key, and model names
- Ollama OpenAI-compatible base URL, API key, text model, and vision model
- OpenAI base URL, API key, text model, and vision model
- Codex, Claude, and Grok executable path, args, timeout, and reasoning effort

### Update Checks

On launch the app makes a single, anonymous request to the GitHub Releases API
(`api.github.com/repos/k3snyder/document-summarizer/releases/latest`) to see
whether a newer version has been published. When one is available, an **Update**
button appears in the top bar; clicking it shows the release notes and links to
the [Releases page](https://github.com/k3snyder/document-summarizer/releases) so
you can download the new build. "Skip this version" hides the button until a
newer release ships.

This is the only network call the app makes on its own. It sends no document
content, prompts, or personal data — only an anonymous `GET` with a
`document-summarizer/<version>` User-Agent. Turn it off any time under
**Settings → Updates → "Check for updates on launch"** (on by default); the
preference is stored in `settings.json` as `updates.enabled`.

## Usage Details

Desktop app data is stored under `~/.summarizer` (macOS/Linux) and `%USERPROFILE%\.summarizer` (Windows):

- Settings: `~/.summarizer/settings.json`
- History index: `~/.summarizer/history.json`
- Job JSON output: `~/.summarizer/jobs/<job_id>/output.json`
- Job Markdown output: `~/.summarizer/jobs/<job_id>/output.md`
- Logs: `~/.summarizer/logs/` (`job-<job_id>.jsonl`, `desktop.jsonl`, and legacy date files)

Settings, history, job output, and log files are written with `0600` permissions on macOS/Linux. The history view reloads completed jobs after an app restart.

<details>
<summary><strong>Provider readiness & status behavior</strong></summary>

The sidebar status is app-level state: `Ready` means the app is idle and can start a job; `processing` means a job is running. Individual provider health is shown beside each provider option in the Process wizard. The app re-checks selected providers before starting a job and blocks the run if a required provider is offline. llama.cpp readiness checks `/v1/models` first and falls back to `/health`; Codex, Claude, and Grok readiness resolve the configured executable and run `--version`.

Step 2 Vision Processing defaults to Codex, including legacy `settings.json` files that omit `pipeline_defaults`. CLI providers expose configurable reasoning effort in Desktop Settings.

When the packaged app launches outside a terminal, it searches common CLI locations in addition to the inherited `PATH`: `/opt/homebrew/bin`, `/usr/local/bin`, system binary directories, `~/.cargo/bin`, `~/.local/bin`, `~/.nvm/current/bin`, installed `~/.nvm/versions/node/*/bin` directories, and `/Applications/Codex.app/Contents/Resources` on macOS/Linux; and `%APPDATA%\npm`, `%NVM_HOME%`, `%NVM_SYMLINK%`, `%LOCALAPPDATA%\Microsoft\WindowsApps`, `%ProgramFiles%\nodejs`, and standard LibreOffice install directories on Windows. Windows lookup also checks `.exe`, `.cmd`, `.bat`, and `.com`. Setting an absolute executable path in Settings is the most deterministic option for packaged builds.

PPTX vision processing renders every slide to a screenshot via LibreOffice/`soffice`, then sends those images through the selected vision provider. If `soffice` is unavailable and PPTX vision is enabled, the app blocks the job before queueing instead of failing mid-run.

DOCX support is native `.docx`/OpenXML extraction. Legacy binary `.doc` files are not supported. DOCX output is chunked by document structure rather than fixed visual pages, so Word documents appear as logical chunks/sections. Headings, lists, tables, headers, footers, footnotes, endnotes, comments, and embedded images are folded into page/chunk text, tables, and image fields. Embedded DOCX images can feed the vision stage when image extraction is enabled.

</details>

<details>
<summary><strong>Headless job submission</strong> (enqueue into a running instance)</summary>

There is no HTTP server. To submit a job from an external tool or AI agent, launch the app binary while an instance is already running. `tauri-plugin-single-instance` forwards the arguments to the live window, so the job lands in the in-app queue and History and is processed with the user's configured providers.

```bash
# App already open. Enqueue a document into the running instance:
document-summarizer-desktop --enqueue /path/to/document.pdf

# Optional pipeline override (full PipelineConfig JSON):
document-summarizer-desktop --enqueue /path/to/document.pdf \
  --config-json '{"vision_mode":"none","summarizer_provider":"llama_cpp"}'
```

On macOS the bundled binary lives at `/Applications/Document Summarizer.app/Contents/MacOS/document-summarizer-desktop` (or use `open -a "Document Summarizer" --args --enqueue <file>`).

| Flag | Description |
|------|-------------|
| `--enqueue <path>` / `-e <path>` | File to enqueue (repeatable for batches) |
| `--config-json <json>` | Optional `PipelineConfig` override applied to every file |

Without `--config-json`, jobs use the desktop default pipeline config plus the user's saved provider settings. Completion writes `~/.summarizer/jobs/<id>/output.json`; job state is tracked in `~/.summarizer/history.json`.

</details>

## Development

```bash
# Primary checks
make fmt-check
make lint
make test
cd apps/desktop && npm ci && npm run build && npm run tauri:build
```

`make test` runs the backend workspace. PDF integration tests are gated behind `RUN_PDFIUM_TESTS=1` and require `PDFIUM_LIB_PATH`; local runs print a visible skip notice when the gate is unset, and CI enables the gate.

<details>
<summary><strong>Common verification commands</strong></summary>

```bash
cargo test --manifest-path backend-rs/Cargo.toml
cargo test --manifest-path apps/desktop/src-tauri/Cargo.toml
npm --prefix apps/desktop run build
npm --prefix apps/desktop run tauri:build
```

Testing convention: mock external LLM/vision HTTP services with `wiremock` so CI stays offline and cost-free. Test internal extraction, parsing, transformation, and schema logic with real fixtures rather than mocks.

CI (`.github/workflows/ci.yml`) runs on pushes and PRs to `main` plus a weekly schedule: a Linux job (rustfmt, clippy with `-D warnings`, and workspace tests with PDFium enabled), a macOS job (desktop Rust tests, frontend build, and `tauri:build`), and a continue-on-error security audit (`cargo audit` for both Rust trees and `npm audit` for the desktop frontend).

</details>

<details>
<summary><strong>Build commands</strong></summary>

Build the macOS app bundle:

```bash
cd apps/desktop
npm run tauri:build
open "src-tauri/target/release/bundle/macos/Document Summarizer.app"
```

Build the unsigned Windows NSIS installer on Windows:

```powershell
npm --prefix apps/desktop ci
npm --prefix apps/desktop run tauri:build:windows
```

</details>

<details>
<summary><strong>Release process</strong></summary>

Releases use `vX.Y.Z` git tags and keep all app/package manifests on the same semantic version. Do not create a release tag from a dirty worktree.

1. Pick the release version, then run `node scripts/bump-version.mjs X.Y.Z`.
2. Update `CHANGELOG.md`: move the relevant `[Unreleased]` notes under the new version heading and leave a fresh `[Unreleased]` section.
3. Confirm `apps/desktop/src-tauri/resources/pdfium/PDFIUM_VERSION.txt` records the bundled PDFium artifact and checksum for the build being shipped.
4. Run the verification gates:

```bash
make fmt-check
make lint
make test
cargo test --manifest-path apps/desktop/src-tauri/Cargo.toml
npm --prefix apps/desktop run lint
npm --prefix apps/desktop run build
npm --prefix apps/desktop run tauri:build
git diff --check
```

5. Commit the release changes, then create the annotated tag:

```bash
git commit -m "Release vX.Y.Z"
git tag -a vX.Y.Z -m "vX.Y.Z"
```

6. Build the release DMG from the tagged commit with `.docs/scripts/build-release.sh` on macOS.
7. For Windows validation builds, run `.docs/scripts/build-release-windows.ps1` on Windows. This produces an unsigned NSIS setup executable and SHA-256 checksum files.

</details>

## Project Structure

```
.
├── apps/
│   └── desktop/                # Tauri 2 desktop app with embedded pipeline
├── backend-rs/
│   ├── crates/
│   │   ├── summarizer-types
│   │   ├── summarizer-extraction
│   │   ├── summarizer-vision
│   │   ├── summarizer-summarization
│   │   ├── summarizer-cli-util
│   │   └── summarizer-pipeline
│   └── prompts/                # LLM prompt templates
├── scripts/                    # bump-version.mjs
└── .github/workflows/ci.yml    # Rust, macOS desktop, and audit CI jobs
```

## Tech Stack

- **Desktop**: Tauri 2, React 19, TypeScript 5.9, Vite 7, plain CSS
- **Pipeline**: Rust 1.88, Tokio, `summarizer-pipeline` embedded in the desktop app
- **Extraction**: pdfium-render/pdfium-auto, ZIP/XML parsing for PPTX and DOCX
- **Providers**: llama.cpp, Ollama, OpenAI, Codex CLI, Claude CLI, Grok CLI

## Status & Scope

Document Summarizer is a **desktop app for a single workstation**. With a local model (llama.cpp / Ollama) it can run fully offline, keeping documents and model output on your machine; cloud providers are also supported. It is not intended to be installed, distributed, or operated as a public/multi-user service.

- **Supported platform**: macOS on Apple Silicon (the Tauri desktop app in `apps/desktop/`, with the Rust pipeline embedded in the app process).
- **Windows**: an unsigned NSIS internal build lane exists for validation, but should not be treated as a public supported release target until the clean Windows smoke checklist has passed.
- **Rendering**: the app currently renders document and model output as plain text. Tauri CSP hardening is intentionally deferred for this local-only app, but must be revisited before any view renders document- or LLM-derived HTML, or if `load_settings` secrets become reachable by rendered content.

## License

[MIT](LICENSE) © k3snyder
