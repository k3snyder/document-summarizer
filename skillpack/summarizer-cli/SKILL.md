---
name: summarizer-cli
description: Run a local document through Document Summarizer and retrieve the structured output. Use when an agent needs to process a PDF, PPTX, DOCX, TXT, or Markdown file through the app's Rust pipeline. Prefer the headless `summarizer-cli` binary when available; use the desktop `--enqueue` fallback when the user specifically wants the job in the live app queue and History.
---

# Summarizer CLI

## Overview

Document Summarizer is a standalone Tauri/Rust desktop app with a shared Rust
pipeline. It exposes **no HTTP API**. For agents, this skill prefers the
standalone `summarizer-cli` binary: one blocking invocation writes
`<stem>_output.json` next to the input and prints a JSON manifest. If the CLI is
not available, or if the user asks to watch the app, the skill can fall back to
the desktop `--enqueue` path, which forwards the job into the live app queue and
History.

**Backend rule:** `--backend auto` is the default and uses CLI when found,
otherwise the app enqueue path. `--backend cli` requires no app or display
session. `--backend app` requires the desktop app to be open and is the only
path that writes to app History.

**This skill is interactive: always ask the user how to process the file
(step 2) and wait for their answer before enqueuing.** Do not auto-pick a
favorite or default just because no options were mentioned.

## Workflow

### 1. Pick a backend

Default to `--backend auto`. It resolves the headless `summarizer-cli` binary
from `--cli-bin`, `SUMMARIZER_CLI_BIN`, the repo's `backend-rs/target`, or
`PATH`; if not found, it falls back to the desktop app path.

Use `--backend cli` when the app is closed, when running in botcky/launchd, or
when the user wants the simple `{stem}_output.json` artifact. Use
`--backend app` when the user wants the job visible in the app queue/History; in
that mode the desktop app must already be running.

Before the first processing run in a session, or after any provider/PDFium
failure, run a preflight:

```bash
python3 scripts/run_job.py --doctor
```

Use `--doctor --favorite <name-or-number>` or add the same custom flags you plan
to run so the check validates the merged effective config. Doctor never runs the
pipeline and never writes an output file.

### 2. Choose how to process — ALWAYS ASK FIRST (blocking)

**STOP. This is a required, blocking gate. You MUST present the favorites menu
and wait for the user's reply before enqueuing anything.** Do not run
`run_job.py` to process the file yet (only `--list-favorites` is allowed at this
point). "Process this file" with no further detail is **not** permission to pick
for the user — it means *show the menu and ask*. Never silently default to
`default`.

The ONLY time you may skip the menu is when the user, in their own message, has
**already** named a favorite ("use vision-every-page") or given explicit
processing instructions ("…and skip the tables"). In every other case — including
a bare "process this" — you must ask.

#### Automation callers (cron, CI, multi-agent — no user to ask)

The gate counts as **pre-answered** only when:

1. the user named a favorite or gave explicit processing instructions
   in-message (the interactive rule above), or
2. the invocation comes from an automated context whose configuration names the
   favorite — the favorite in the automation config *is* the user's answer.
   Pass it explicitly: `--favorite <name>`.

A bare automated call with no favorite must **fail loudly, never default**.
Automation wrappers should set `--require-favorite`, which makes `run_job.py`
exit 2 (naming `--list-favorites`) when no `--favorite` was supplied.

Read the list at runtime so user-added favorites show up:

```bash
python3 scripts/run_job.py --list-favorites
```

Then present a numbered menu (Custom last) and ask the user to choose. For example:

> How should I process **processmap.pptx**?
> 1. **Default** — full pipeline with the app's configured providers
> 2. **Vision extraction on every page** — skip the classifier; extract every rendered page
> 3. **Custom** — answer a few quick questions to tailor the run
>
> Reply **1**, **2**, or **3** (or name a favorite).

**Wait for the reply.** Only after the user answers:

- **1 / "default"** → `python3 scripts/run_job.py --file <path> --favorite default`
- **2 / a named favorite** → `python3 scripts/run_job.py --file <path> --favorite <name-or-number>`
  (a favorite's flags merge onto the desktop default; explicit flags you add still override)
- **3 / "custom"** → run the guided Q&A below, assemble the flags, confirm, then run.

For documents likely over 25 pages, offer a dry-run estimate before the user
commits to a heavy option:

```bash
python3 scripts/run_job.py --file /path/to/document.pdf --estimate [same flags/favorite]
```

The estimate reports page/chunk count, stage plan, expected provider calls, and
the effective config. It does not call model providers or write output.

Favorites live in `favorites.json` at the skill root (`name`, `title`,
`description`, `flags[]` of plain `run_job.py` flags). The user can add their
own; offer to save a useful custom set as a new favorite.

#### Custom: guided Q&A

Ask only what's relevant; treat "default" / "skip" / no answer as "leave it at
the app default" (add no flag). Walk these, then confirm the assembled command:

1. **Depth** — full pipeline (default) / extraction only, no models
   (`--extract-only --no-run-summarization --vision-mode none`) / text only
   (`--text-only`)?
2. **Vision** — on (default) or off (`--vision-mode none`)? If on: every page
   (`--vision-skip-classification`)? A specific provider (`--vision-mode <p>`)?
   Separate classifier/extractor (`--vision-classifier-mode <x> --vision-extractor-mode <y>`)?
3. **Summaries** — default / detailed 3× (`--summarizer-detailed-extraction`) /
   add insights (`--summarizer-insight-mode`) / topics only
   (`--summarizer-mode topics-only`) / skip (`--no-run-summarization`)? Provider
   (`--summarizer-provider <p>`)?
4. **Extraction extras** — skip tables (`--skip-tables`) / skip images
   (`--skip-images`) / DPI (`--pdf-image-dpi 72|144|200|300`)?

Show the user the exact `run_job.py` command you assembled, get a yes, then run.

**Direct mapping** — if the user already described the behavior (menu skipped) or
you're building a Custom run, translate intent to flags. Overrides merge **onto
the desktop default** (Vision = Codex, summarizer = Codex), so a single flag
won't silently disable the rest of the pipeline. Most useful mappings (full list
in `references/config-schema.md`):

| If the user says (intent) | Add this flag |
| --- | --- |
| "run extraction on every rendered page", "disable classification", "don't classify, just extract every page", "process every page visually" | `--vision-skip-classification` |
| "extraction only", "just extract text, no models", "don't run vision or summaries" | `--extract-only --no-run-summarization --vision-mode none` |
| "text only", "just the text", "no tables or images" | `--text-only` |
| "skip tables", "ignore tables", "don't extract tables" | `--skip-tables` |
| "skip images", "ignore images", "don't pull out images" | `--skip-images` |
| "skip the tables in the PowerPoint" | `--skip-pptx-tables` |
| "higher resolution / sharper page images", "render at 300 DPI", "lower DPI / faster" | `--pdf-image-dpi 300` (or `72`/`144`/`200`) |
| "no vision", "skip the image analysis" | `--vision-mode none` |
| "use llama.cpp / Ollama / OpenAI / Claude / Grok for summaries" | `--summarizer-provider <name>` |
| "use Codex/Claude/Grok CLI for vision" | `--vision-mode <name>` (and `--vision-cli-provider <name>` if needed) |
| "use X to classify and Y to extract", "advanced vision with separate providers" | `--vision-classifier-mode <x> --vision-extractor-mode <y>` |
| "detailed / thorough / deeper summaries", "extract 3x and synthesize" | `--summarizer-detailed-extraction` |
| "add insights", "extra insight pass" | `--summarizer-insight-mode` (needs `--summarizer-mode full`) |
| "topics only", "skip the summaries" | `--summarizer-mode topics-only` / `--summarizer-mode skip` |
| "more time / bigger token budget per page", "raise the per-page limits" | `--max-seconds-per-page N` / `--max-tokens-per-page N` |
| "bigger / smaller chunks" | `--chunk-size N` / `--chunk-overlap N` |
| "keep the page images in the output" | `--keep-base64-images` |

Each boolean flag has a `--no-…` form to force it off (e.g.
`--no-vision-skip-classification`, `--no-skip-tables`). Run
`python3 scripts/run_job.py --help` for the full grouped list, or pass an
explicit `--config-json '{...}'` partial object for anything not covered; see
`references/config-schema.md`.

### 3. Enqueue and wait

Only reach this step **after** the user has chosen an option in step 2 (or
explicitly specified processing in their request). Run the command that matches
their choice — e.g. for the Default favorite:

```bash
python3 scripts/run_job.py --file /path/to/document.pdf --favorite default
```

With the default CLI backend, the script:

- resolves `summarizer-cli` (`--cli-bin`, `SUMMARIZER_CLI_BIN`, repo
  `backend-rs/target/{release,debug}`, then `PATH`)
- runs `summarizer-cli <file> --config-json <partial> --output <stem>_output.json --quiet`
- waits for the blocking process to finish
- prints the CLI manifest with `"backend": "cli"`

With `--backend app`, the script:

- resolves the app binary (`--app-bin`, then `SUMMARIZER_APP_BIN`, then the repo
  `target/{release,debug}`, then the macOS bundle)
- snapshots existing job ids in `~/.summarizer/history.json`
- runs `<app-bin> --enqueue <file> [--config-json …]` (forwarded to the running app)
- polls `history.json` until the new job reaches `completed` / `failed` / `canceled`
- on success, reads `~/.summarizer/jobs/<id>/output.json` and prints a manifest

### 4. Read the result

The printed manifest includes `job_id`, `status`, `output_json_path`, the
`backend`, and usually the `document` metadata and `page_count`. Read
results through `query_result.py` by default so only the needed fields enter
context:

```bash
python3 scripts/query_result.py --input /path/to/<stem>_output.json --summary
python3 scripts/query_result.py --input /path/to/<stem>_output.json --pages 12 --fields text,summary_notes,summary_topics
python3 scripts/query_result.py --input /path/to/<stem>_output.json --topics
python3 scripts/query_result.py --input /path/to/<stem>_output.json --grep "revenue"
python3 scripts/query_result.py --input /path/to/<stem>_output.json --tables --as csv --out /tmp/tables
```

`query_result.py` strips `image_base64` and embedded image payloads unless
`--include-images` is explicitly set. Read raw `output_json_path` only as an
escape hatch when the compact selectors cannot answer the question. CLI jobs do
**not** appear in the app's History; use `--backend app` for that.

### 4a. Scale and background runs

Use batch mode when the user asks for a folder or multiple files. The favorites
gate is asked once for the whole batch:

```bash
python3 scripts/run_job.py --dir /path/to/docs --glob '*.pdf' --recursive --favorite default --parallel 2
python3 scripts/run_job.py --files a.pdf b.docx c.md --favorite default
```

CLI runs are cataloged in `~/.summarizer/cli-runs.jsonl`. Repeating an unchanged
file with the same effective config returns a cached manifest; use `--force` to
rerun or `--no-cache` for throwaway runs. Inspect cataloged runs with:

```bash
python3 scripts/run_job.py --list-runs --limit 10
python3 scripts/query_result.py --run latest --summary
```

Use detach when an estimate exceeds roughly 8 minutes or the user wants to keep
working while a run continues:

```bash
python3 scripts/run_job.py --file long.pdf --favorite default --detach
python3 scripts/run_job.py --status <run_id>
python3 scripts/run_job.py --wait <run_id>
python3 scripts/run_job.py --cancel <run_id>
```

### 4b. Page sampling and exports

For expensive documents, run a sample first, inspect it, then ask before running
the full document:

```bash
python3 scripts/run_job.py --file long.pdf --favorite default --sample 3
python3 scripts/query_result.py --input long_output.json --summary
```

Use explicit `--pages '1-3,8,10'` when the user names page ranges. Export
structured assets through `query_result.py`:

```bash
python3 scripts/query_result.py --input output.json --export tables --export pages-jsonl --out exports/
python3 scripts/query_result.py --input output.json --include-images --export images --out exports/
```

### 4c. Corpus synthesis

After processing multiple CLI runs, create a deterministic corpus brief:

```bash
python3 scripts/synthesize.py --latest 3 --out corpus-brief.md
```

`--llm` is optional and best-effort; the default brief uses only existing
notes/topics/metadata and performs no model calls.

### 5. Failure handling

- Headless CLI exits non-zero → inspect the manifest `error`; provider settings,
  PDFium, or the selected provider may be unavailable.
- Enqueue command exits non-zero → the app is probably not running; ask the user
  to open it or rerun with `--backend cli`.
- Job ends `failed` → inspect the manifest's `final_job.error`; the configured provider may be unreachable.
- Wrong binary → pass `--cli-bin` / `SUMMARIZER_CLI_BIN`, or for app mode
  `--app-bin` / `SUMMARIZER_APP_BIN`.
- Automation wrappers should pass `--require-favorite`; a bare automated call
  without `--favorite` must fail loudly instead of choosing a default.

### 6. Convert a result to OKF (on request)

When the user says **"convert output to OKF"**, "export as OKF", or "make an OKF
bundle", run `scripts/to_okf.py` on a finished job's `output.json`. It writes
Google's Open Knowledge Format (OKF v0.1) — markdown with YAML frontmatter and
per-slide/per-page content — and prints a JSON manifest of written paths. This
is a read-only transform; it does not re-process the document.

Pick the source the same way the user refers to it:

```bash
# Newest finished app job (only app-History jobs; CLI outputs need --input):
python3 scripts/to_okf.py --latest

# A specific job, or an explicit output.json:
python3 scripts/to_okf.py --job-id <job_id>
python3 scripts/to_okf.py --input /path/to/<stem>_output.json --out /path/to/dest
```

- **Default (`--granularity pages`)** emits a conformant **OKF directory bundle**:
  `index.md` (carries `okf_version`), `document.md` (the whole-document concept),
  and `pages/page-NNNN.md` (one concept per slide/page with `type: Slide` /
  `Document Page`, topics, notes, visual extraction, extracted text, tables).
  Lands in `<output.json dir>/okf/<slug>/` unless `--out` is given.
- **`--granularity single`** emits one self-contained `<slug>.okf.md` with
  document frontmatter and a `## Slide/Page N` section per page (a convenience
  export, not a conformant bundle). Use it when the user wants "a single file".
- `.pptx`/`.ppt` → "Slide"; other types → "Page" (auto; override with
  `--page-type`/`--doc-type`). `--no-log` skips the bundle's `log.md`.

Report the bundle dir / file path from the manifest so the user can open it.

## Examples

Use the app's defaults (full pipeline with the user's providers):

```bash
python3 scripts/run_job.py --file /path/to/document.pdf
```

Extraction only (no model providers needed):

```bash
python3 scripts/run_job.py --file /path/to/document.pdf \
  --extract-only --no-run-summarization --vision-mode none
```

Summarization on llama.cpp, vision off:

```bash
python3 scripts/run_job.py --file /path/to/document.md \
  --vision-mode none --summarizer-provider llama_cpp
```

Advanced vision — run extraction on every rendered page (skip the classifier):

```bash
python3 scripts/run_job.py --file /path/to/document.pdf \
  --vision-skip-classification
```

Run a favorite preset (by name or number) — same as above via favorites:

```bash
python3 scripts/run_job.py --list-favorites
python3 scripts/run_job.py --file /path/to/document.pdf --favorite vision-every-page
```

Convert the newest job's output to an OKF bundle ("convert output to OKF"):

```bash
python3 scripts/to_okf.py --latest
# single-file variant:
python3 scripts/to_okf.py --latest --granularity single
```

Exact override with JSON:

```bash
python3 scripts/run_job.py --file /path/to/document.pptx \
  --config-json '{"vision_mode":"codex","summarizer_provider":"codex"}'
```

Point at an explicit binary (e.g. installed macOS app):

```bash
SUMMARIZER_APP_BIN="/Applications/Document Summarizer.app/Contents/MacOS/document-summarizer-desktop" \
  python3 scripts/run_job.py --file /path/to/document.pdf
```

## Resources

### `scripts/run_job.py`

Resolve the app binary, forward the file with `--enqueue`, watch History for
completion, and print a manifest plus the output path. Supports per-stage
override flags, `--favorite <name|number>`, and `--list-favorites`.

### `favorites.json`

Editable list of favorite presets (`name`, `title`, `description`, `flags[]`)
shown in the favorites menu and resolved by `--favorite`. Add user presets here.

### `scripts/to_okf.py`

Convert a finished job's `output.json` into Google's Open Knowledge Format
(OKF v0.1): a markdown directory bundle (default) or a single file
(`--granularity single`), with YAML frontmatter and per-slide/per-page content.
Sourced by `--input` / `--job-id` / `--latest`; prints a JSON manifest.

### `references/api-contract.md`

The `--enqueue` CLI contract, single-instance forwarding behavior, and the
`~/.summarizer` artifact layout the script reads.

### `references/config-schema.md`

Current `PipelineConfig` keys, defaults, and common provider choices.
