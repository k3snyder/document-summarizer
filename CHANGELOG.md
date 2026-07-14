# Changelog

All notable changes to Document Summarizer are recorded here.

This project follows semantic versioning for release tags and app package
versions. Keep `apps/desktop/package.json`,
`apps/desktop/src-tauri/tauri.conf.json`, `apps/desktop/src-tauri/Cargo.toml`,
and the backend crate manifests on the same version.

## [Unreleased]

## [0.4.1] - 2026-07-14

### Changed

- Reorganized the Settings page: the CLI provider block (Codex, Claude, Grok,
  Copilot) now leads the provider sections and is expanded by default,
  Appearance and Updates share a single row, the Logging and Summarization
  Budget sections moved to the bottom, and the provider-visibility grids
  (Vision, Classifier, Summarizer) list providers in a consistent order —
  Codex CLI, llama.cpp, Copilot CLI, OpenAI, Ollama, Grok CLI, Claude CLI —
  without changing the Process wizard's provider ordering.
- Reordered the Page Details sections in the output viewer to Text, Tables,
  Image Text (with the classifier verdict beneath the extracted text),
  Embedded Images, Summary Notes, Topics, and Detailed Summary Attempts. The
  Warnings, Embedded Images, and Detailed Summary Attempts blocks now render
  only when they have content instead of showing empty placeholders.
- Bumped the backend crate manifests to the release version; they had
  remained at `0.1.0` since the initial release.

### Fixed

- CLI providers (Codex, Claude, Grok, Copilot) no longer flag full-document
  summaries with a "Summary quality validation not reached" warning: the
  relevancy/quality loop only exists for OpenAI-compatible providers, so CLI
  summaries now report no validation state rather than `false`. The
  unvalidated badge additionally requires a numeric relevancy score (shared
  `summary-quality.ts` predicate with unit tests), so legacy CLI outputs stop
  showing the badge retroactively.
- Fixed the sidebar footer (status pill and Settings button) being pushed
  below the fold by tall History content: the app shell is now capped at the
  viewport height, the sidebar stays pinned, and only the workspace pane
  scrolls.

## [0.4.0] - 2026-07-14

### Added

- Added a standalone headless `summarizer-cli` binary that runs the full
  extraction/vision/summarization pipeline with no GUI: `--config-json`,
  repeatable `--set key=value`, `--output`, `--markdown`, `--settings`,
  `--env-providers`, `--pdfium`, `--job-id`, and `--quiet`. It emits a single
  JSON manifest on stdout with deterministic exit codes (`0` completed, `1`
  pipeline failure, `2` usage/config, `3` environment) and resolves PDFium
  explicitly (flag, `SUMMARIZER_PDFIUM`, installed app resources, dev tree) —
  never via implicit download.
- Bundled `summarizer-cli` into the macOS app as a sidecar under
  `Contents/Resources/resources/bin/`, so installed agent skills can resolve
  it without a repo checkout. `npm run tauri:build` builds and stages it
  automatically via `apps/desktop/scripts/prepare-cli-resource.sh`.
- Added Codex CLI model selection: a `Codex model` dropdown in Settings
  (CLI default, `gpt-5.6-sol`, `gpt-5.6-terra`, `gpt-5.6-luna`, `gpt-5.5`)
  and a `CODEX_CLI_MODEL` environment override for headless runs. Custom
  Codex args always take precedence, and `--model` is never emitted twice.
- Surfaced the actual model used (not just the provider) in output metrics
  (`metrics.config.vision_model` / `summarizer_model`), job logs, and the
  Processing Metrics provider tiles, preferring the provider-reported model
  over the configured one.
- Added `--doctor` (settings/PDFium/provider preflight with local endpoint
  probes) and `--estimate` (provider-free page/stage/call-count and time-band
  projection) modes to the CLI and agent skill.
- Added page-range and sampling support (`page_range` config; `--pages` /
  `--sample` in the skill) threaded through PDF, PPTX, DOCX, TXT, and
  Markdown extraction.
- Expanded the `summarizer-cli` agent skillpack into a full workbench:
  CLI-first execution with app-enqueue fallback (`--backend auto|cli|app`), an
  append-only run catalog with content-hash caching
  (`~/.summarizer/cli-runs.jsonl`), batch processing
  (`--dir`/`--files`/`--glob`/`--parallel`), detached jobs
  (`--detach`/`--status`/`--wait`/`--cancel`), result queries and exports
  (`query_result.py`), OKF passthrough (`--okf`), corpus briefs
  (`synthesize.py`), a corpus dataset compiler (`to_dataset.py`), an installer
  (`install_skill.sh`), and a stdlib-only test harness.
- Added `to_okf.py`: converts a summarizer `output.json` into a Google Open
  Knowledge Format (OKF v0.1) directory bundle or single markdown file.

### Changed

- Moved provider settings and CLI argument construction into the shared
  pipeline crate (`summarizer_pipeline::settings`) so the desktop app and the
  headless CLI use identical provider configuration, and partial config
  overrides merge onto the desktop defaults everywhere.
- Changed the default Codex CLI model to `gpt-5.6-terra` for fresh installs
  and for legacy settings files that predate the model field; an explicit
  "CLI default" (empty) selection is preserved.
- CLI runs are recorded in the CLI catalog rather than the desktop app's
  History; use `--backend app` when History visibility is wanted.

### Security

- Added an image decompression-bomb guard to PPTX/DOCX image extraction: a
  header-only dimension check (16-million-pixel cap) rejects oversized images
  before decode, mirroring the existing vision-path guard.
- Hardened project CI: the `GITHUB_TOKEN` is restricted to read-only contents,
  and the PDFium test-library download is SHA-256-pinned before extraction.

## [0.3.0] - 2026-06-30

### Added

- Added a GitHub Copilot CLI provider (`copilot`) for both vision and
  summarization, alongside `codex`, `claude`, and `grok`, with its own
  executable/args/timeout settings and provider-visibility toggles.
- Added an on-launch update check that queries the GitHub Releases API and shows
  an `Update` button in the top bar when a newer version is published. The dialog
  surfaces release notes with `View release`, `Download DMG`, `Later`, and
  `Skip this version` actions (Tauri commands `check_for_update`,
  `skip_update_version`, and `app_version`).
- Added a `Settings → Updates` section with a `Check for updates on launch`
  toggle (on by default) and a current-version display, persisted as
  `updates.enabled` and `updates.skipped_version` in `settings.json`.
- Added the Tauri `opener` capability (`opener:default`, `opener:allow-open-url`)
  so the update dialog can open release and download URLs in the browser.
- Documented the update-check behavior, opt-out, and its network/privacy
  footprint in the README.

### Changed

- Changed the default Ollama summarizer model from `llama3.2` to
  `gemma4:12b-it-qat` (the vision model stays `llava`).

### Fixed

- Fixed `--config-json` to merge a partial override onto the desktop default
  pipeline config instead of resetting omitted fields, so flipping one toggle no
  longer silently disables vision; non-object `--config-json` payloads are now
  rejected.
- Stopped showing the file-upload prompt while a job is actively processing.

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

[Unreleased]: https://github.com/k3snyder/document-summarizer/compare/v0.4.1...HEAD
[0.4.1]: https://github.com/k3snyder/document-summarizer/compare/v0.4.0...v0.4.1
[0.4.0]: https://github.com/k3snyder/document-summarizer/compare/v0.3.0...v0.4.0
[0.3.0]: https://github.com/k3snyder/document-summarizer/compare/v0.2.0...v0.3.0
[0.2.0]: https://github.com/k3snyder/document-summarizer/compare/v0.1.0...v0.2.0
[0.1.0]: https://github.com/k3snyder/document-summarizer/releases/tag/v0.1.0
