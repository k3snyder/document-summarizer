# Changelog

All notable changes to Document Summarizer are recorded here.

This project follows semantic versioning for release tags and app package
versions. Keep `apps/desktop/package.json`,
`apps/desktop/src-tauri/tauri.conf.json`, `apps/desktop/src-tauri/Cargo.toml`,
and the backend crate manifests on the same version.

## [Unreleased]

## [0.2.0] - 2026-06-25

### Added

- Added automatic retry with exponential backoff for CLI vision and
  summarization providers: timed-out, crashed, or failed-to-spawn `codex`,
  `claude`, and `grok` invocations are re-attempted (3 tries by default,
  configurable per provider via `CODEX_CLI_RETRIES`, `CLAUDE_CLI_RETRIES`, and
  `GROK_CLI_RETRIES`).
- Added a release-process checklist and version bump helper.
- Added desktop ESLint coverage and TypeScript unused-local enforcement.
- Added bundled PDFium provenance notes in
  `apps/desktop/src-tauri/resources/pdfium/PDFIUM_VERSION.txt`.
- Added structured quality-gate observability with per-page validation status.
- Added shared OPC package handling for PPTX and DOCX relationship parsing,
  package path normalization, and embedded image conversion.
- Added desktop log subscription cleanup support.
- Added `--enqueue <file>` headless job submission: relaunching the desktop
  binary forwards the document to the already-running instance via
  `tauri-plugin-single-instance`, so the job lands in the live queue and History
  and is processed with the user's configured providers. Supports repeatable
  `--enqueue` and an optional `--config-json` pipeline override.

### Changed

- Desktop runs providers from explicit settings passed into the embedded pipeline
  instead of mutating process environment variables.
- Vision image normalization now runs off the async runtime and uses a bounded
  cache with page-level release hooks.
- README now states the current release boundary: macOS Apple Silicon packaged
  app support, local-first operation, PDFium test gating, and the CSP tripwire.

### Fixed

- Improved timeout recovery for vision and summarization: a single CLI provider
  timeout or crash no longer aborts the whole job. Failed pages are retried and,
  if still unsuccessful, degrade gracefully (continuing without that page's
  summary, classification, or image text) instead of failing the document.
- Fixed relevancy score parsing for common judge formats such as `8/10`,
  `92%`, and `0.85`.
- Fixed Gemini vision authentication so API keys are sent by header instead of
  URL query string.
- Added HTTP provider timeouts so wedged providers fail instead of hanging jobs
  indefinitely.
- Added actual decompressed-byte accounting for ZIP/OpenXML extraction.
- Serialized PDFium use for concurrent server jobs.
- Ensured server temp and output directories are created with private
  permissions on Unix.
- Ensured CLI subprocesses are killed when their owning command future is
  dropped.
- Removed the broken detailed vision synthesis path while continuing to accept
  the deprecated config field for compatibility.
- Removed the deprecated Next.js frontend and the server CORS layer it required.

### Removed

- Removed the legacy Axum REST server (`summarizer-server`) and HTTP CLI client
  (`summarizer-cli`) crates, their Dockerfile, the `scripts/smoke-cli.sh` smoke
  check, and the `services-*` Makefile targets. The desktop app is the sole
  product surface; headless automation goes through `--enqueue`.

## [0.1.0]

### Added

- Initial Rust/Tauri desktop document summarizer with PDF, PPTX, DOCX, TXT, and
  Markdown extraction.
- Embedded Rust pipeline with extraction, vision classification/extraction, and
  summarization stages.
- Provider support for llama.cpp, Ollama, OpenAI-compatible APIs, Codex CLI,
  Claude CLI, and Grok CLI.
- Local desktop settings, job history, JSON/Markdown outputs, and redacted job
  logs under `~/.summarizer`.
- Headless Axum server and HTTP CLI for local workflow automation.

[Unreleased]: https://github.com/k3snyder/document-summarizer/compare/v0.2.0...HEAD
[0.2.0]: https://github.com/k3snyder/document-summarizer/compare/v0.1.0...v0.2.0
[0.1.0]: https://github.com/k3snyder/document-summarizer/releases/tag/v0.1.0
