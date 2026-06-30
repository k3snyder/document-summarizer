# Enqueue Contract

Document Summarizer is a standalone Tauri/Rust desktop app with **no HTTP API**.
Jobs are submitted by relaunching the app binary; `tauri-plugin-single-instance`
forwards the arguments to the already-running instance.

Canonical source files in the repo:
- `apps/desktop/src-tauri/src/lib.rs` — `parse_cli_enqueue`, `enqueue_jobs_core`, the single-instance plugin registration, `DesktopJob`/`DesktopJobStatus`
- `backend-rs/crates/summarizer-types/src/lib.rs` — `PipelineConfig`, `DocumentOutput`

## CLI

```
document-summarizer-desktop --enqueue <path> [--enqueue <path> ...] [--config-json <json>]
```

- `--enqueue <path>` / `-e <path>`: file to enqueue; repeatable for batches. Also `--enqueue=<path>`.
- `--config-json <json>`: optional `PipelineConfig` override applied to every file. Also `--config-json=<json>`. When omitted, the desktop default config is used. A partial object is **merged onto the desktop default** (vision/summarizer = `codex`), so keys you omit keep that default rather than resetting to the bare type default — flip one toggle (e.g. `vision_skip_classification`) without disabling the rest. Must decode to a JSON object.

Behavior:
- If an instance is already running, the new process forwards its argv to it and exits immediately (exit 0). The running app enqueues the job(s) into the live queue + History and focuses its window.
- If no instance is running, the binary starts as the first instance and enqueues the same args at startup. In a headless shell (no display) a fresh GUI launch will not succeed, so an instance must already be open.

## Artifact layout (`~/.summarizer`)

The script reads app state directly from disk (home dir resolved from `$HOME`, or `SUMMARIZER_HOME`):

- `history.json` — JSON array of `DesktopJob` records (newest first). Fields used:
  - `job_id`: string
  - `status`: `queued` | `processing` | `completed` | `failed` | `canceled` (snake_case)
  - `file_name`: string
  - `error`: nullable string (populated on `failed`)
  - `duration_ms`, `created_at`, `completed_at`, …
  - `output` is `null` here; completed output lives in the file below.
- `jobs/<job_id>/output.json` — the completed `DocumentOutput` (`document` + `pages[]`).
- `settings.json` — provider configuration; the running app uses this for endpoints/models. The enqueue path never sends provider endpoints — those come from the app's saved settings, which is what makes a forwarded job run with the user's full capabilities.

## Polling pattern

1. Snapshot existing `job_id`s in `history.json`.
2. Run `<app-bin> --enqueue <file> [--config-json …]`.
3. Poll `history.json` for a new `job_id` whose `file_name` matches, until its `status` is terminal.
4. On `completed`, read `jobs/<job_id>/output.json`.

## Notes

- The forwarding process returns within ~1s; the actual processing happens asynchronously in the running app, so always poll History rather than relying on the command's exit.
- There is no Markdown endpoint; the desktop app generates Markdown on demand via its UI (`save_job_markdown`). Agents should consume `output.json`.
