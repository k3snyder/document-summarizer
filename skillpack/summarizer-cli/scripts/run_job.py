#!/usr/bin/env python3
"""
Submit a document to the running Document Summarizer desktop app and wait for
the result.

The desktop app has no HTTP server. This script relaunches the app binary with
`--enqueue <file>`; `tauri-plugin-single-instance` forwards the argument to the
already-running instance, which drops the job into the live queue + History and
processes it with the user's configured providers. The script then watches
`~/.summarizer/history.json` for the new job to finish and reads its
`~/.summarizer/jobs/<id>/output.json`.

Prerequisite: the desktop app must already be running.
"""

from __future__ import annotations

import argparse
import json
import os
import subprocess
import sys
import time
from pathlib import Path
from typing import Any


VISION_CHOICES = [
    "none", "deepseek", "gemini", "openai", "ollama", "llama_cpp", "codex", "claude", "grok",
]
SUMMARIZER_CHOICES = ["ollama", "llama_cpp", "openai", "codex", "claude", "grok"]
SUMMARIZER_MODE_CHOICES = ["full", "topics-only", "skip"]
CLI_CHOICES = ["codex", "claude", "grok"]
DPI_CHOICES = [72, 144, 200, 300]
TERMINAL_STATUSES = {"completed", "failed", "canceled"}

# macOS bundle location for an installed app.
MACOS_BUNDLE_BIN = (
    "/Applications/Document Summarizer.app/Contents/MacOS/document-summarizer-desktop"
)
BIN_NAME = "document-summarizer-desktop"

# Built-in favorites, used when favorites.json is missing or unreadable. The
# on-disk favorites.json (skill root) is the editable source of truth; users can
# add their own presets there.
DEFAULT_FAVORITES = [
    {
        "name": "default",
        "title": "Default",
        "description": (
            "Process with the app's full default pipeline (vision + summarization "
            "on the user's configured providers). No overrides."
        ),
        "flags": [],
    },
    {
        "name": "vision-every-page",
        "title": "Vision extraction on every page",
        "description": "Skip the vision classifier and run visual extraction on every rendered page.",
        "flags": ["--vision-skip-classification"],
    },
]


class JobClientError(RuntimeError):
    """Raised for expected client-side failures."""


def eprint(message: str) -> None:
    print(message, file=sys.stderr)


def summarizer_home() -> Path:
    # The app resolves its data dir from the home directory; honor an override so
    # tests/sandboxes can isolate state, matching the app's use of $HOME.
    home = os.environ.get("SUMMARIZER_HOME") or os.environ.get("HOME")
    if not home:
        raise JobClientError("Could not determine home directory (set HOME or SUMMARIZER_HOME).")
    return Path(home).expanduser() / ".summarizer"


def favorites_path() -> Path:
    # Editable favorites list at the skill root (scripts/ -> skill root).
    override = os.environ.get("SUMMARIZER_FAVORITES")
    if override:
        return Path(override).expanduser()
    return Path(__file__).resolve().parent.parent / "favorites.json"


def load_favorites() -> list[dict[str, Any]]:
    """Load the favorites list, falling back to the built-in defaults if the
    file is missing or malformed."""
    path = favorites_path()
    if path.is_file():
        try:
            data = json.loads(path.read_text(encoding="utf-8"))
        except (json.JSONDecodeError, OSError):
            return DEFAULT_FAVORITES
        favorites = data.get("favorites") if isinstance(data, dict) else None
        if isinstance(favorites, list) and favorites:
            return [fav for fav in favorites if isinstance(fav, dict)]
    return DEFAULT_FAVORITES


def find_favorite(selector: str) -> dict[str, Any] | None:
    """Resolve a favorite by 1-based number or by name (case-insensitive)."""
    favorites = load_favorites()
    selector = selector.strip()
    if selector.isdigit():
        index = int(selector) - 1
        return favorites[index] if 0 <= index < len(favorites) else None
    lowered = selector.lower()
    for favorite in favorites:
        if str(favorite.get("name", "")).lower() == lowered:
            return favorite
    return None


def resolve_app_bin(explicit: str | None) -> str:
    candidates: list[str] = []
    if explicit:
        candidates.append(explicit)
    if os.environ.get("SUMMARIZER_APP_BIN"):
        candidates.append(os.environ["SUMMARIZER_APP_BIN"])
    # Dev builds inside the repo.
    repo_root = Path(__file__).resolve()
    for parent in repo_root.parents:
        target = parent / "apps" / "desktop" / "src-tauri" / "target"
        if target.is_dir():
            candidates.append(str(target / "release" / BIN_NAME))
            candidates.append(str(target / "debug" / BIN_NAME))
            break
    candidates.append(MACOS_BUNDLE_BIN)

    for candidate in candidates:
        if candidate and Path(candidate).is_file() and os.access(candidate, os.X_OK):
            return candidate
    raise JobClientError(
        "Could not locate the desktop app binary. Set SUMMARIZER_APP_BIN or pass --app-bin. "
        f"Tried: {', '.join(candidates)}"
    )


def read_history(home: Path) -> list[dict[str, Any]]:
    history_path = home / "history.json"
    if not history_path.is_file():
        return []
    try:
        data = json.loads(history_path.read_text(encoding="utf-8"))
    except (json.JSONDecodeError, OSError):
        return []
    return data if isinstance(data, list) else []


def snapshot_job_ids(home: Path) -> set[str]:
    ids: set[str] = set()
    for job in read_history(home):
        job_id = job.get("job_id") if isinstance(job, dict) else None
        if isinstance(job_id, str):
            ids.add(job_id)
    return ids


def forward_enqueue(app_bin: str, file_path: Path, config_json: str | None, timeout: float) -> None:
    command = [app_bin, "--enqueue", str(file_path)]
    if config_json is not None:
        command += ["--config-json", config_json]
    try:
        completed = subprocess.run(
            command, capture_output=True, text=True, timeout=timeout, check=False
        )
    except subprocess.TimeoutExpired as exc:
        raise JobClientError(
            "The app binary did not return after forwarding. Is an instance running?"
        ) from exc
    # The forwarding (second) instance exits immediately after handing args to the
    # running app. A non-zero exit usually means no instance was running and a
    # fresh GUI launch was attempted in an environment without a display.
    if completed.returncode not in (0, None):
        detail = (completed.stderr or completed.stdout or "").strip()
        raise JobClientError(
            f"Enqueue command exited with code {completed.returncode}. "
            f"Ensure the desktop app is already running. {detail}".strip()
        )


def wait_for_new_job(
    home: Path,
    before: set[str],
    file_name: str,
    poll_interval: float,
    timeout: float,
) -> dict[str, Any]:
    started = time.time()
    job_id: str | None = None
    last_line: str | None = None
    while True:
        if time.time() - started > timeout:
            raise JobClientError(
                f"Timed out after {timeout:.0f}s waiting for a job for {file_name}."
            )
        jobs = read_history(home)
        if job_id is None:
            # Newest jobs are inserted at the front of the list.
            for job in jobs:
                jid = job.get("job_id")
                if jid and jid not in before and job.get("file_name") == file_name:
                    job_id = jid
                    eprint(f"job_id={job_id}")
                    break
        if job_id is not None:
            job = next((j for j in jobs if j.get("job_id") == job_id), None)
            if job is not None:
                status = job.get("status")
                line = f"[{status}]"
                if line != last_line:
                    eprint(line)
                    last_line = line
                if status in TERMINAL_STATUSES:
                    return job
        time.sleep(poll_interval)


def build_config_json(args: argparse.Namespace) -> str | None:
    """Build a PipelineConfig override, or None to use the app's own default
    (desktop default config + the user's saved provider settings)."""
    if args.config_json:
        try:
            loaded = json.loads(args.config_json)
        except json.JSONDecodeError as exc:
            raise JobClientError(f"--config-json is not valid JSON: {exc}") from exc
        if not isinstance(loaded, dict):
            raise JobClientError("--config-json must decode to a JSON object")
    else:
        loaded = {}

    explicit = {
        # Extraction
        "extract_only": args.extract_only,
        "text_only": args.text_only,
        "skip_tables": args.skip_tables,
        "skip_images": args.skip_images,
        "skip_pptx_tables": args.skip_pptx_tables,
        "pdf_image_dpi": args.pdf_image_dpi,
        "chunk_size": args.chunk_size,
        "chunk_overlap": args.chunk_overlap,
        # Vision
        "vision_mode": args.vision_mode,
        "vision_classifier_mode": args.vision_classifier_mode,
        "vision_extractor_mode": args.vision_extractor_mode,
        "vision_cli_provider": args.vision_cli_provider,
        "vision_skip_classification": args.vision_skip_classification,
        # Summarization
        "run_summarization": args.run_summarization,
        "summarizer_mode": args.summarizer_mode,
        "summarizer_provider": args.summarizer_provider,
        "summarizer_cli_provider": args.summarizer_cli_provider,
        "summarizer_detailed_extraction": args.summarizer_detailed_extraction,
        "summarizer_insight_mode": args.summarizer_insight_mode,
        "max_tokens_per_page": args.max_tokens_per_page,
        "max_seconds_per_page": args.max_seconds_per_page,
        # Output
        "keep_base64_images": args.keep_base64_images,
    }
    for key, value in explicit.items():
        if value is not None:
            loaded[key] = value

    if not loaded:
        return None
    return json.dumps(loaded, separators=(",", ":"), ensure_ascii=True)


def build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(
        description="Enqueue a document into the running Document Summarizer app and wait for output."
    )
    parser.add_argument("--file", help="Path to a PDF, PPTX, DOCX, TXT, or MD document (required unless --list-favorites)")
    parser.add_argument("--favorite", help="Apply a saved favorite preset by name or number (see --list-favorites)")
    parser.add_argument("--list-favorites", action="store_true", help="Print the saved favorite presets as JSON and exit")
    parser.add_argument("--app-bin", help="Path to the desktop app binary (default: SUMMARIZER_APP_BIN, repo target, or the macOS bundle)")
    parser.add_argument("--config-json", help="JSON PipelineConfig override; omit to use the app's default + saved settings")
    parser.add_argument("--poll-interval", type=float, default=2.0, help="History polling interval in seconds")
    parser.add_argument("--timeout-seconds", type=float, default=3600.0, help="Total time to wait for completion")
    parser.add_argument("--enqueue-timeout-seconds", type=float, default=30.0, help="Time to wait for the forwarding command to return")

    # --- Extraction (Stage 1) ---
    extraction = parser.add_argument_group("extraction")
    extraction.add_argument(
        "--extract-only", action=argparse.BooleanOptionalAction, default=None,
        help="Stop after extraction (skip vision + summarization); no model providers needed",
    )
    extraction.add_argument(
        "--text-only", action=argparse.BooleanOptionalAction, default=None,
        help="Extract text only (skip tables and images)",
    )
    extraction.add_argument(
        "--skip-tables", action=argparse.BooleanOptionalAction, default=None,
        help="Do not extract tables",
    )
    extraction.add_argument(
        "--skip-images", action=argparse.BooleanOptionalAction, default=None,
        help="Do not extract embedded images",
    )
    extraction.add_argument(
        "--skip-pptx-tables", action=argparse.BooleanOptionalAction, default=None,
        help="Do not extract tables from PPTX slides",
    )
    extraction.add_argument(
        "--pdf-image-dpi", type=int, choices=DPI_CHOICES, default=None,
        help="Render DPI for PDF page images fed to vision (72/144/200/300; higher = sharper, slower)",
    )
    extraction.add_argument(
        "--chunk-size", type=int, default=None, help="Text chunk size",
    )
    extraction.add_argument(
        "--chunk-overlap", type=int, default=None, help="Text chunk overlap",
    )

    # --- Vision (Stage 2) ---
    vision = parser.add_argument_group("vision")
    vision.add_argument(
        "--vision-mode", choices=VISION_CHOICES, default=None,
        help="Vision provider, or 'none' to disable Stage 2 vision",
    )
    vision.add_argument(
        "--vision-classifier-mode", choices=VISION_CHOICES, default=None,
        help="Advanced mode: separate provider for page classification",
    )
    vision.add_argument(
        "--vision-extractor-mode", choices=VISION_CHOICES, default=None,
        help="Advanced mode: separate provider for visual extraction",
    )
    vision.add_argument(
        "--vision-cli-provider", choices=CLI_CHOICES, default=None,
        help="CLI provider for vision when --vision-mode is a CLI",
    )
    vision.add_argument(
        "--vision-skip-classification", action=argparse.BooleanOptionalAction, default=None,
        help="Run extraction on every rendered page (skip the vision classifier step)",
    )

    # --- Summarization (Stage 3) ---
    summarization = parser.add_argument_group("summarization")
    summarization.add_argument(
        "--run-summarization", action=argparse.BooleanOptionalAction, default=None,
        help="Run Stage 3 summarization",
    )
    summarization.add_argument(
        "--summarizer-mode", choices=SUMMARIZER_MODE_CHOICES, default=None,
        help="Summary depth: full | topics-only | skip",
    )
    summarization.add_argument(
        "--summarizer-provider", choices=SUMMARIZER_CHOICES, default=None,
        help="Summarization provider",
    )
    summarization.add_argument(
        "--summarizer-cli-provider", choices=CLI_CHOICES, default=None,
        help="CLI provider for summarization when --summarizer-provider is a CLI",
    )
    summarization.add_argument(
        "--summarizer-detailed-extraction", action=argparse.BooleanOptionalAction, default=None,
        help="Run extraction 3x per page and synthesize (slower, more thorough)",
    )
    summarization.add_argument(
        "--summarizer-insight-mode", action=argparse.BooleanOptionalAction, default=None,
        help="Extra insight pass (only active with --summarizer-mode full)",
    )
    summarization.add_argument(
        "--max-tokens-per-page", type=int, default=None,
        help="Per-page summarization token budget (default 100000)",
    )
    summarization.add_argument(
        "--max-seconds-per-page", type=int, default=None,
        help="Per-page summarization time budget in seconds (default 300)",
    )

    # --- Output ---
    output = parser.add_argument_group("output")
    output.add_argument(
        "--keep-base64-images", action=argparse.BooleanOptionalAction, default=None,
        help="Keep base64 page images in the output JSON (large)",
    )
    return parser


def main() -> int:
    parser = build_parser()
    args = parser.parse_args()

    if args.list_favorites:
        print(json.dumps({"favorites": load_favorites()}, indent=2))
        return 0

    if args.favorite:
        favorite = find_favorite(args.favorite)
        if favorite is None:
            raise JobClientError(
                f"Unknown favorite '{args.favorite}'. Run --list-favorites to see options."
            )
        favorite_flags = [str(flag) for flag in favorite.get("flags", [])]
        if favorite_flags:
            # Re-parse with the favorite's flags as the base so any explicit CLI
            # flags (which appear later in argv) override the favorite.
            args = parser.parse_args(favorite_flags + sys.argv[1:])
        eprint(f"favorite={favorite.get('name')} flags={favorite_flags}")

    if not args.file:
        raise JobClientError("--file is required (unless --list-favorites).")

    file_path = Path(args.file).expanduser().resolve()
    if not file_path.is_file():
        raise JobClientError(f"File not found: {file_path}")

    app_bin = resolve_app_bin(args.app_bin)
    home = summarizer_home()
    config_json = build_config_json(args)

    before = snapshot_job_ids(home)
    eprint(f"app_bin={app_bin}")
    eprint(f"summarizer_home={home}")
    forward_enqueue(app_bin, file_path, config_json, args.enqueue_timeout_seconds)

    job = wait_for_new_job(
        home, before, file_path.name, args.poll_interval, args.timeout_seconds
    )
    job_id = job["job_id"]

    manifest: dict[str, Any] = {
        "job_id": job_id,
        "app_bin": app_bin,
        "summarizer_home": str(home),
        "config_json": config_json,
        "status": job.get("status"),
        "final_job": job,
    }

    if job.get("status") != "completed":
        print(json.dumps(manifest, indent=2, sort_keys=True))
        raise JobClientError(
            f"Job {job_id} ended with status {job.get('status')}: "
            f"{job.get('error') or job.get('message') or 'no detail'}"
        )

    output_json_path = home / "jobs" / job_id / "output.json"
    manifest["output_json_path"] = str(output_json_path)
    if output_json_path.is_file():
        try:
            output = json.loads(output_json_path.read_text(encoding="utf-8"))
            manifest["document"] = output.get("document")
            manifest["page_count"] = len(output.get("pages", []))
        except (json.JSONDecodeError, OSError) as exc:
            manifest["output_read_error"] = str(exc)
    else:
        manifest["output_read_error"] = "output.json not found"

    print(json.dumps(manifest, indent=2, sort_keys=True))
    return 0


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except JobClientError as exc:
        eprint(f"ERROR: {exc}")
        raise SystemExit(1)
