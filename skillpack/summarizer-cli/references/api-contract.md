# Backend Contract

Document Summarizer has no HTTP API. Agents use one of two local process
surfaces:

1. `summarizer-cli` — preferred headless binary; blocking; writes output where
   requested; does not touch app History.
2. `document-summarizer-desktop --enqueue` — desktop fallback; forwards into a
   running app instance; jobs appear in live queue and History.

Canonical source files:
- `backend-rs/crates/summarizer-cli/src/main.rs` — headless CLI contract
- `apps/desktop/src-tauri/src/lib.rs` — `parse_cli_enqueue`, `enqueue_jobs_core`,
  single-instance plugin registration, `DesktopJob`/`DesktopJobStatus`
- `backend-rs/crates/summarizer-types/src/lib.rs` — `PipelineConfig`,
  `DocumentOutput`, `PipelineConfig::merge_json_onto_desktop_default`

## Headless CLI

```bash
summarizer-cli <INPUT>
  --config-json '{...}'
  --set key=value
  --output <path>
  --markdown
  --settings <path>
  --env-providers
  --pdfium <path>
  --job-id <id>
  --quiet

summarizer-cli --doctor [<INPUT>] [--config-json '{...}'] [--settings <path>|--env-providers]
summarizer-cli <INPUT> --estimate [--config-json '{...}'] [--pdfium <path>]
summarizer-cli --print-config [--config-json '{...}'] [--set key=value]
```

- `--config-json`: partial `PipelineConfig` object, merged onto
  `PipelineConfig::desktop_default()` (vision/summarizer = `codex`).
- `--set key=value`: repeatable top-level config override, applied after
  `--config-json`; values parse as JSON scalars, falling back to strings.
- `--output`: defaults to `<input dir>/<stem>_output.json`.
- `--markdown`: also writes `<stem>_output.md` using `DocumentOutput::to_markdown`.
- `--settings`: provider settings file; defaults to
  `$SUMMARIZER_HOME/.summarizer/settings.json` or `$HOME/.summarizer/settings.json`.
- `--env-providers`: skip settings and use `PipelineProviderConfig::from_env()`.
- `--pdfium`: explicit PDFium dylib/so/dll path for PDF/PPTX.
- `--quiet`: suppress progress lines on stderr.
- `--doctor`: validates settings, PDFium when relevant, and the providers used
  by the effective config. It writes no output file and exits `0` only when all
  non-skipped checks pass.
- `--estimate`: probes document structure and reports stage/call counts without
  constructing a pipeline or invoking providers.
- `--print-config`: prints the effective config JSON after desktop-default merge
  and `--set` overrides; no input file is required.

Stdout is exactly one JSON manifest line at the end:

```json
{
  "job_id": "...",
  "status": "completed",
  "input": "/path/input.pdf",
  "output_json_path": "/path/input_output.json",
  "output_md_path": "/path/input_output.md",
  "document": {
    "document_id": "...",
    "filename": "input.pdf",
    "total_pages": 12
  },
  "duration_ms": 12345
}
```

On failure, `status` is `"failed"` and `error` is present.

Doctor stdout:

```json
{
  "doctor": true,
  "cli_version": "0.1.0",
  "checks": [
    {"name": "settings", "status": "ok", "detail": "..."},
    {"name": "pdfium", "status": "skip", "detail": "..."}
  ],
  "ok": true
}
```

Estimate stdout:

```json
{
  "estimate": true,
  "pages": 7,
  "per_page_chars": [1200],
  "per_page_tables": [0],
  "stages": {"extraction": true, "vision": false, "summarization": true},
  "per_stage_calls": {"classify": 0, "vision_extract": 0, "summarize": 7},
  "budget_band_seconds": {"max": 2100, "source": "budget-derived"},
  "effective_config": {}
}
```

Exit codes:
- `0`: completed
- `1`: pipeline/job/output write failure
- `2`: usage/config error (including non-object `--config-json`)
- `3`: environment error (missing input, malformed settings, unresolved PDFium)

Stderr progress lines, unless `--quiet`:

```text
[1/3] extraction 2/10 20% Extracting page 2
```

## Skill Scripts

`scripts/run_job.py` wraps both backends. Additional non-processing modes:

- `--doctor`: combines skill-level checks (`summarizer-cli` resolution, app
  binary resolution, favorites loading) with `summarizer-cli --doctor`.
- `--estimate --file <path>`: calls the headless CLI dry-run and prints its JSON
  report with `backend`, `cli_bin`, and `config_json` fields added.

`scripts/query_result.py` is the default read path for finished outputs:

```bash
python3 scripts/query_result.py --input output.json --summary
python3 scripts/query_result.py --input output.json --pages 2-4 --fields text,summary_notes
python3 scripts/query_result.py --input output.json --grep "revenue"
python3 scripts/query_result.py --input output.json --topics
python3 scripts/query_result.py --input output.json --stats
python3 scripts/query_result.py --input output.json --tables --as csv --out tables/
```

It accepts `--input`, `--job-id`, or `--latest`; emits compact JSON by default;
supports `--md`; and strips `image_base64` / embedded image payloads unless
`--include-images` is set.

Additional script contracts:

- `run_job.py --list-runs [--limit N] [--for <path>]` reads the append-only CLI
  catalog at `~/.summarizer/cli-runs.jsonl`.
- CLI-backend runs are cached by `(input_sha256, effective_config_hash)` unless
  `--force` or `--no-cache` is set. The app backend is never cataloged.
- `run_job.py --dir <folder> [--glob PATTERN] [--recursive]` and repeatable
  `--files` process batches through the CLI backend only and return one batch
  manifest.
- `run_job.py --detach` returns a detached run handle. `--status`, `--wait`, and
  `--cancel` operate on `~/.summarizer/cli-jobs/<run_id>/`.
- `run_job.py --pages '1-3,8'` sets `PipelineConfig.page_range`; `--sample N`
  estimates page count and sets a front/middle/back sample range.
- `run_job.py --okf` runs `to_okf.py` after successful single or batch items and
  merges `bundle_dir` / `file_written` into the manifest.
- `query_result.py --export tables|pages-jsonl|images --out <dir>` writes CSV,
  JSONL, and decoded image assets.
- `synthesize.py --latest N|--runs ...|--for GLOB` writes a deterministic
  corpus brief from cataloged completed CLI runs.
- `scripts/install_skill.sh` mirrors the skill to `~/.codex/skills/` and
  `~/.claude/skills/`.

## PDFium Resolution

For `.pdf` and `.pptx`, the headless CLI resolves PDFium in this order:

1. `--pdfium`
2. `SUMMARIZER_PDFIUM`
3. installed app resource locations
4. dev tree fallback:
   `apps/desktop/src-tauri/resources/pdfium/<platform-lib>`
5. hard error listing tried paths

It does not silently fall through to `pdfium-auto` network download.

Platform library names:
- macOS: `libpdfium.dylib`
- Linux: `libpdfium.so`
- Windows: `pdfium.dll`

## Desktop Enqueue

```bash
document-summarizer-desktop --enqueue <path> [--enqueue <path> ...] [--config-json <json>]
```

- `--enqueue <path>` / `-e <path>`: repeatable file to enqueue. Also
  `--enqueue=<path>`.
- `--config-json <json>`: partial `PipelineConfig`, merged onto desktop default.

Behavior:
- If an instance is already running, the new process forwards its argv and exits.
  The running app enqueues jobs into live queue + History and focuses its window.
- If no instance is running, the binary starts as the first GUI instance. In a
  headless shell this usually fails; use `summarizer-cli` instead.

## App Artifact Layout (`~/.summarizer`)

The app backend reads state from disk:

- `history.json`: JSON array of `DesktopJob` records, newest first.
- `jobs/<job_id>/output.json`: completed `DocumentOutput`.
- `settings.json`: provider configuration used by the running app.

Polling pattern:

1. Snapshot existing `job_id`s in `history.json`.
2. Run `<app-bin> --enqueue <file> [--config-json ...]`.
3. Poll `history.json` for a new matching terminal job.
4. On `completed`, read `jobs/<job_id>/output.json`.

## History Difference

Headless CLI jobs do not appear in app History and do not write
`~/.summarizer/jobs/`. This avoids races with the desktop app's wholesale
History rewrites and makes concurrent agent runs safe. Use `--backend app` when
History visibility is required.
