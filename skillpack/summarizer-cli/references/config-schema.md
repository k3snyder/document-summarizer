# Config Schema

Current pipeline config shape. Canonical source of truth:
- `backend-rs/crates/summarizer-types/src/lib.rs` (`PipelineConfig` + the `VisionMode` / `SummarizerProvider` / `SummarizerMode` / `CliProvider` / `PdfImageDpi` enums)

Every field is optional — each has `#[serde(default)]`, so a partial config
object is valid.

How the base is chosen (headless `--enqueue` path):
- **Omit `--config-json` entirely** → the desktop app uses its own default,
  `desktop_default_pipeline_config()` (vision **and** summarizer = `codex`), plus
  the user's saved provider settings. This is "process as normal with full
  capabilities".
- **Send a partial `--config-json`** → it is merged **onto that same desktop
  default**. Only the keys you set change; every field you omit keeps the
  desktop default (vision/summarizer = `codex`). So you can flip a single
  advanced toggle (e.g. `vision_skip_classification`) without disabling vision
  or changing the summarizer. (Send an explicit `vision_mode`/`summarizer_*` if
  you want something other than `codex`.)

## Bare type defaults (`PipelineConfig::default()`)

These are the field-level `#[serde(default)]` values of the type. They are the
defaults you get from the canonical library API — but on the `--enqueue` path a
partial override merges onto `desktop_default_pipeline_config()` (codex/codex),
**not** onto this table, so omitting `vision_mode` leaves it at `codex`, not
`none`.

```json
{
  "run_extraction": true,
  "extract_only": false,
  "skip_tables": false,
  "skip_images": false,
  "skip_pptx_tables": false,
  "text_only": false,
  "pdf_image_dpi": 200,
  "vision_mode": "none",
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

Note: `vision_mode` defaults to `none` in the bare type, but the `--enqueue`
merge base is `codex` (see above). To force vision off, send `vision_mode=none`
explicitly; to keep the desktop default, omit it.

## Field summary

Every field below has a matching friendly flag: the same name in
`--kebab-case`. Booleans use `--flag` / `--no-flag` (e.g. `--skip-tables` /
`--no-skip-tables`); enums and numbers take a value (e.g. `--vision-mode codex`,
`--pdf-image-dpi 300`, `--max-seconds-per-page 600`). Run
`python3 scripts/run_job.py --help` for the grouped list. Flags are the
preferred way to override; `--config-json` is the escape hatch.

### Extraction

- `run_extraction`: run Stage 1 extraction (default true)
- `extract_only`: stop after extraction (skips vision + summarization)
- `skip_tables`
- `skip_images`
- `skip_pptx_tables`
- `text_only`
- `pdf_image_dpi`: `72 | 144 | 200 | 300` (accepts the number `200` or the string `"200"`)

### Vision

- `vision_mode`: `none | deepseek | gemini | openai | ollama | llama_cpp | codex | claude | grok`
- `vision_classifier_mode`: optional override, same `VisionMode` choices
- `vision_extractor_mode`: optional override, same `VisionMode` choices
- `vision_cli_provider`: `codex | claude | grok`, optional
- `vision_skip_classification`: skip the classify step and extract every page

> Note: Gemini is supported by the Rust vision crate but blocked in the desktop
> host. `vision_detailed_extraction` is deprecated — still accepted for backward
> compatibility but ignored, so do not set it.

### Summarization

- `run_summarization`
- `summarizer_mode`: `full | topics-only | skip` (kebab-case on the wire)
- `summarizer_provider`: `ollama | llama_cpp | openai | codex | claude | grok`
- `summarizer_cli_provider`: `codex | claude | grok`, optional
- `summarizer_detailed_extraction`: run extraction 3x and synthesize
- `summarizer_insight_mode`: extra insight pass (only active with `summarizer_mode=full`)
- `max_tokens_per_page`: per-page summarization token budget (default 100000)
- `max_seconds_per_page`: per-page summarization time budget in seconds (default 300)

### Output

- `keep_base64_images`: keep base64 page images in the output JSON (large)

## Provider notes

- Primary local provider is **llama.cpp** (used for both vision and summarization); **Ollama** is the fallback local provider.
- `openai`, `codex`, `claude`, and `grok` are also available; CLI providers (`codex`/`claude`/`grok`) require the matching CLI to be installed and on PATH for the desktop app process.
- Vision and summarization require a reachable model endpoint/CLI. If none is configured, use `extract_only` for a pure-extraction run.

## Common recipes

### Local full pipeline (vision + summary on llama.cpp)

```json
{
  "vision_mode": "llama_cpp",
  "summarizer_provider": "llama_cpp",
  "summarizer_mode": "full"
}
```

### Summarization only (no vision)

```json
{
  "vision_mode": "none",
  "summarizer_provider": "llama_cpp",
  "summarizer_mode": "full"
}
```

### Extraction only (no model endpoint required)

```json
{
  "extract_only": true,
  "run_summarization": false,
  "vision_mode": "none"
}
```

### CLI vision extraction

```json
{
  "vision_mode": "codex",
  "vision_cli_provider": "codex"
}
```

### Advanced vision: run extraction on every rendered page

Skip the classifier step so every rendered page goes through visual extraction.
Because partial overrides merge onto the desktop default, this single key keeps
vision on `codex`; no need to also send `vision_mode`.

```json
{
  "vision_skip_classification": true
}
```

Equivalent flag: `--vision-skip-classification` (or `--no-vision-skip-classification`
to force it off).

When the user needs exact control, prefer the friendly flags
(`--vision-mode`, `--vision-skip-classification`, `--summarizer-provider`, …) or
a `--config-json '{...}'` object over guessing.
