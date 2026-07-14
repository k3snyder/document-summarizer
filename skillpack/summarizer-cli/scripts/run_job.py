#!/usr/bin/env python3
"""
Run a document through Document Summarizer and wait for the result.

By default this script prefers the headless `summarizer-cli` binary, which runs
the full pipeline without a GUI app or display session and writes
`<stem>_output.json` next to the input. If the CLI is unavailable, it falls back
to the desktop app enqueue path: relaunch the app binary with `--enqueue <file>`
and watch `~/.summarizer/history.json` for completion.
"""

from __future__ import annotations

import argparse
import concurrent.futures
import hashlib
import json
import os
import re
import signal
import shutil
import subprocess
import sys
import tempfile
import threading
import time
import uuid
from datetime import datetime, timezone
from pathlib import Path
from typing import Any

from _catalog import append_run, config_hash as hash_config, file_sha256, find_cached, find_runs
import to_okf


VISION_CHOICES = [
    "none", "deepseek", "gemini", "openai", "ollama", "llama_cpp", "codex", "claude", "grok", "copilot",
]
SUMMARIZER_CHOICES = ["ollama", "llama_cpp", "openai", "codex", "claude", "grok", "copilot"]
SUMMARIZER_MODE_CHOICES = ["full", "topics-only", "skip"]
CLI_CHOICES = ["codex", "claude", "grok", "copilot"]
DPI_CHOICES = [72, 144, 200, 300]
TERMINAL_STATUSES = {"completed", "failed", "canceled"}
SUPPORTED_EXTS = {".pdf", ".pptx", ".docx", ".txt", ".md", ".markdown"}
_children: set[subprocess.Popen[str]] = set()
_children_lock = threading.Lock()

# macOS bundle location for an installed app.
MACOS_BUNDLE_BIN = (
    "/Applications/Document Summarizer.app/Contents/MacOS/document-summarizer-desktop"
)
MACOS_BUNDLE_CLI = (
    "/Applications/Document Summarizer.app/Contents/Resources/resources/bin/summarizer-cli"
)
USER_MACOS_BUNDLE_CLI = (
    "~/Applications/Document Summarizer.app/Contents/Resources/resources/bin/summarizer-cli"
)
BIN_NAME = "document-summarizer-desktop"
CLI_BIN_NAME = "summarizer-cli"

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


def resolve_cli_bin(explicit: str | None) -> str | None:
    candidates: list[str] = []
    if explicit:
        candidates.append(explicit)
    if os.environ.get("SUMMARIZER_CLI_BIN"):
        candidates.append(os.environ["SUMMARIZER_CLI_BIN"])
    repo_root = Path(__file__).resolve()
    for parent in repo_root.parents:
        target = parent / "backend-rs" / "target"
        if target.is_dir():
            candidates.append(str(target / "release" / CLI_BIN_NAME))
            candidates.append(str(target / "debug" / CLI_BIN_NAME))
            break
    candidates.append(MACOS_BUNDLE_CLI)
    candidates.append(str(Path(USER_MACOS_BUNDLE_CLI).expanduser()))
    which = shutil.which(CLI_BIN_NAME)
    if which:
        candidates.append(which)

    for candidate in candidates:
        if candidate and Path(candidate).is_file() and os.access(candidate, os.X_OK):
            return candidate
    return None


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


def default_cli_output_path(file_path: Path) -> Path:
    return file_path.with_name(f"{file_path.stem}_output.json")


def append_cli_options(command: list[str], args: argparse.Namespace) -> None:
    if args.settings:
        command += ["--settings", args.settings]
    if args.env_providers:
        command.append("--env-providers")
    if args.pdfium:
        command += ["--pdfium", args.pdfium]


def register_child(process: subprocess.Popen[str]) -> None:
    with _children_lock:
        _children.add(process)


def unregister_child(process: subprocess.Popen[str]) -> None:
    with _children_lock:
        _children.discard(process)


def terminate_children() -> None:
    with _children_lock:
        children = list(_children)
    for process in children:
        if process.poll() is None:
            process.terminate()
    deadline = time.time() + 5
    for process in children:
        remaining = max(deadline - time.time(), 0.1)
        try:
            process.wait(timeout=remaining)
        except subprocess.TimeoutExpired:
            process.kill()


def run_command_streaming(
    command: list[str],
    timeout_seconds: float,
    stderr_prefix: str | None = None,
    status_callback: Any | None = None,
    start_new_session: bool = False,
) -> tuple[int, str, str]:
    process = subprocess.Popen(
        command,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        text=True,
        stdin=subprocess.DEVNULL,
        start_new_session=start_new_session,
    )
    register_child(process)
    stderr_lines: list[str] = []

    def pump_stderr() -> None:
        assert process.stderr is not None
        for line in process.stderr:
            stripped = line.rstrip("\n")
            stderr_lines.append(stripped)
            if status_callback:
                status_callback(stripped)
            if stripped:
                eprint(f"{stderr_prefix}: {stripped}" if stderr_prefix else stripped)

    thread = threading.Thread(target=pump_stderr, daemon=True)
    thread.start()
    try:
        returncode = process.wait(timeout=timeout_seconds)
    except subprocess.TimeoutExpired as exc:
        process.terminate()
        try:
            process.wait(timeout=5)
        except subprocess.TimeoutExpired:
            process.kill()
        raise JobClientError(f"Headless CLI timed out after {timeout_seconds:.0f}s.") from exc
    finally:
        unregister_child(process)
    thread.join(timeout=2)
    stdout = process.stdout.read() if process.stdout is not None else ""
    return returncode, stdout, "\n".join(stderr_lines)


def effective_config_json(cli_bin: str, config_json: str | None) -> str:
    command = [cli_bin, "--print-config"]
    if config_json is not None:
        command += ["--config-json", config_json]
    completed = subprocess.run(command, capture_output=True, text=True, timeout=30, check=False)
    if completed.returncode != 0:
        raise JobClientError(
            f"Could not compute effective config (exit {completed.returncode}): "
            f"{completed.stderr.strip() or completed.stdout.strip()}"
        )
    return completed.stdout.strip()


def cli_version(cli_bin: str) -> str:
    completed = subprocess.run([cli_bin, "--version"], capture_output=True, text=True, timeout=10, check=False)
    if completed.returncode == 0:
        return completed.stdout.strip()
    return "unknown"


def cached_manifest(entry: dict[str, Any]) -> dict[str, Any]:
    return {
        "backend": "cli",
        "cached": True,
        "job_id": entry.get("run_id"),
        "status": "completed",
        "input": entry.get("input_path"),
        "output_json_path": entry.get("output_json_path"),
        "duration_ms": entry.get("duration_ms", 0),
        "page_count": entry.get("page_count"),
        "config_hash": entry.get("config_hash"),
    }


def catalog_entry(
    manifest: dict[str, Any],
    file_path: Path,
    input_hash: str,
    config_digest: str,
    effective_config: str,
    cli_bin: str,
    favorite: str | None,
    flags: list[str],
    detached: bool = False,
) -> dict[str, Any]:
    document = manifest.get("document") if isinstance(manifest.get("document"), dict) else {}
    page_count = document.get("total_pages") or manifest.get("page_count")
    return {
        "run_id": manifest.get("job_id") or str(uuid.uuid4()),
        "timestamp": datetime.now(timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ"),
        "input_path": str(file_path),
        "input_sha256": input_hash,
        "input_bytes": file_path.stat().st_size if file_path.exists() else None,
        "config_hash": config_digest,
        "effective_config_json": effective_config,
        "cli_version": cli_version(cli_bin),
        "backend": "cli",
        "favorite": favorite,
        "flags": flags,
        "output_json_path": manifest.get("output_json_path"),
        "duration_ms": manifest.get("duration_ms"),
        "page_count": page_count,
        "status": manifest.get("status"),
        "detached": detached,
    }


def run_cli(
    cli_bin: str,
    file_path: Path,
    config_json: str | None,
    timeout: float,
    args: argparse.Namespace,
    stderr_prefix: str | None = None,
    status_callback: Any | None = None,
    detached: bool = False,
    job_id: str | None = None,
) -> dict[str, Any]:
    output_path = default_cli_output_path(file_path)
    effective_config = effective_config_json(cli_bin, config_json)
    input_hash = file_sha256(file_path)
    config_digest = hash_config(effective_config)
    if not args.force and not args.no_cache:
        cached = find_cached(input_hash, config_digest)
        if cached:
            return cached_manifest(cached)

    command = [cli_bin, str(file_path), "--output", str(output_path)]
    if job_id:
        command += ["--job-id", job_id]
    if stderr_prefix is None and status_callback is None:
        command.append("--quiet")
    if config_json is not None:
        command += ["--config-json", config_json]
    append_cli_options(command, args)
    returncode, stdout_text, stderr_text = run_command_streaming(
        command,
        timeout,
        stderr_prefix=stderr_prefix,
        status_callback=status_callback,
    )

    stdout = stdout_text.strip()
    try:
        manifest = json.loads(stdout) if stdout else {}
    except json.JSONDecodeError as exc:
        raise JobClientError(
            f"Headless CLI returned non-JSON stdout (exit {returncode}): {stdout}"
        ) from exc

    if not isinstance(manifest, dict):
        raise JobClientError("Headless CLI manifest was not a JSON object.")
    manifest["backend"] = "cli"
    manifest["cli_bin"] = cli_bin
    manifest["config_json"] = config_json
    manifest["config_hash"] = config_digest
    if not args.no_cache:
        append_run(catalog_entry(
            manifest,
            file_path,
            input_hash,
            config_digest,
            effective_config,
            cli_bin,
            args.favorite,
            sys.argv[1:],
            detached=detached,
        ))
    if returncode != 0 or manifest.get("status") != "completed":
        detail = manifest.get("error") or stderr_text.strip() or stdout or "no detail"
        print(json.dumps(manifest, indent=2, sort_keys=True))
        raise JobClientError(f"Headless CLI failed with exit {returncode}: {detail}")
    return manifest


def run_doctor(args: argparse.Namespace, config_json: str | None) -> dict[str, Any]:
    checks: list[dict[str, Any]] = []
    cli_bin = resolve_cli_bin(args.cli_bin)
    if cli_bin:
        checks.append({"name": "skill:cli_bin", "status": "ok", "detail": cli_bin})
    else:
        checks.append({
            "name": "skill:cli_bin",
            "status": "fail",
            "detail": "Could not locate summarizer-cli.",
            "remedy": "Set SUMMARIZER_CLI_BIN or pass --cli-bin.",
        })

    try:
        app_bin = resolve_app_bin(args.app_bin)
        checks.append({"name": "skill:app_bin", "status": "ok", "detail": app_bin})
    except JobClientError as exc:
        checks.append({"name": "skill:app_bin", "status": "skip", "detail": str(exc)})

    try:
        favorites = load_favorites()
        checks.append({
            "name": "skill:favorites",
            "status": "ok" if favorites else "fail",
            "detail": f"{len(favorites)} favorite(s) loaded from {favorites_path()}",
        })
    except Exception as exc:  # defensive: doctor should report, not crash.
        checks.append({"name": "skill:favorites", "status": "fail", "detail": str(exc)})

    if not cli_bin:
        ok = all(check["status"] != "fail" for check in checks)
        return {"doctor": True, "backend": "skill", "checks": checks, "ok": ok}

    command = [cli_bin, "--doctor"]
    if args.file:
        command.append(str(Path(args.file).expanduser()))
    if config_json is not None:
        command += ["--config-json", config_json]
    append_cli_options(command, args)
    completed = subprocess.run(
        command, capture_output=True, text=True, timeout=args.timeout_seconds, check=False
    )
    try:
        report = json.loads(completed.stdout.strip())
    except json.JSONDecodeError as exc:
        raise JobClientError(f"summarizer-cli --doctor returned non-JSON stdout: {completed.stdout}") from exc
    if not isinstance(report, dict):
        raise JobClientError("summarizer-cli --doctor did not return a JSON object.")
    rust_checks = report.get("checks")
    if isinstance(rust_checks, list):
        checks.extend(check for check in rust_checks if isinstance(check, dict))
    ok = completed.returncode == 0 and all(check.get("status") != "fail" for check in checks)
    return {
        "doctor": True,
        "backend": "cli",
        "cli_bin": cli_bin,
        "config_json": config_json,
        "checks": checks,
        "ok": ok,
    }


def run_estimate(
    cli_bin: str,
    file_path: Path,
    config_json: str | None,
    timeout: float,
    args: argparse.Namespace,
) -> dict[str, Any]:
    command = [cli_bin, str(file_path), "--estimate"]
    if config_json is not None:
        command += ["--config-json", config_json]
    append_cli_options(command, args)
    completed = subprocess.run(
        command, capture_output=True, text=True, timeout=timeout, check=False
    )
    stdout = completed.stdout.strip()
    try:
        manifest = json.loads(stdout) if stdout else {}
    except json.JSONDecodeError as exc:
        raise JobClientError(
            f"Headless CLI estimate returned non-JSON stdout (exit {completed.returncode}): {stdout}"
        ) from exc
    if not isinstance(manifest, dict):
        raise JobClientError("Headless CLI estimate was not a JSON object.")
    manifest["backend"] = "cli"
    manifest["cli_bin"] = cli_bin
    manifest["config_json"] = config_json
    if completed.returncode != 0:
        detail = completed.stderr.strip() or stdout or "no detail"
        raise JobClientError(f"Headless CLI estimate failed with exit {completed.returncode}: {detail}")
    return manifest


def sample_page_range(total_pages: int, sample_size: int) -> str:
    sample_size = max(1, min(sample_size, total_pages))
    front_count = (sample_size + 2) // 3
    middle_count = (sample_size + 1) // 3
    back_count = sample_size - front_count - middle_count
    pages: set[int] = set(range(1, front_count + 1))
    if middle_count:
        middle_start = max(1, (total_pages // 2) - (middle_count // 2) + 1)
        pages.update(range(middle_start, min(total_pages, middle_start + middle_count - 1) + 1))
    if back_count:
        pages.update(range(max(1, total_pages - back_count + 1), total_pages + 1))
    while len(pages) < sample_size:
        for page in range(1, total_pages + 1):
            pages.add(page)
            if len(pages) >= sample_size:
                break
    return ",".join(str(page) for page in sorted(pages))


def apply_sample_range(
    cli_bin: str,
    file_path: Path,
    config_json: str | None,
    args: argparse.Namespace,
) -> str | None:
    if args.sample is None:
        return config_json
    estimate = run_estimate(cli_bin, file_path, config_json, args.timeout_seconds, args)
    total_pages = int(estimate.get("pages") or 0)
    if total_pages <= 0:
        raise JobClientError("Could not sample: estimate returned zero pages.")
    page_range = sample_page_range(total_pages, args.sample)
    eprint(f"sample_pages={page_range}")
    data = json.loads(config_json) if config_json else {}
    data["page_range"] = page_range
    return json.dumps(data, separators=(",", ":"), ensure_ascii=True)


def supported_file(path: Path) -> bool:
    return path.suffix.lower() in SUPPORTED_EXTS


def collect_inputs(args: argparse.Namespace) -> tuple[list[Path], list[dict[str, Any]], bool]:
    sources = sum(1 for value in [args.file, args.dir, args.files] if value)
    if sources > 1:
        raise JobClientError("--file, --dir, and --files are mutually exclusive.")
    if args.dir:
        root = Path(args.dir).expanduser().resolve()
        if not root.is_dir():
            raise JobClientError(f"Directory not found: {root}")
        patterns = args.glob or ["*"]
        paths: list[Path] = []
        for pattern in patterns:
            iterator = root.rglob(pattern) if args.recursive else root.glob(pattern)
            paths.extend(path for path in iterator if path.is_file())
        unique = sorted(set(paths))
        skipped = [
            {"input": str(path), "status": "skipped", "reason": "unsupported"}
            for path in unique
            if not supported_file(path)
        ]
        return [path for path in unique if supported_file(path)], skipped, True
    if args.files:
        raw_paths = [item for group in args.files for item in group]
        paths = [Path(item).expanduser().resolve() for item in raw_paths]
        skipped = [
            {"input": str(path), "status": "skipped", "reason": "unsupported"}
            for path in paths
            if path.exists() and not supported_file(path)
        ]
        return [path for path in paths if supported_file(path)], skipped, True
    if args.file:
        return [Path(args.file).expanduser().resolve()], [], False
    return [], [], False


def process_batch(
    cli_bin: str,
    files: list[Path],
    skipped: list[dict[str, Any]],
    config_json: str | None,
    args: argparse.Namespace,
) -> dict[str, Any]:
    started = time.time()
    max_workers = max(1, min(args.parallel, 4))
    results: list[dict[str, Any] | None] = [None] * len(files)
    interrupted = False

    def handle_signal(signum: int, _frame: Any) -> None:
        nonlocal interrupted
        interrupted = True
        terminate_children()
        raise KeyboardInterrupt(f"received signal {signum}")

    def run_index(index: int, path: Path) -> dict[str, Any]:
        if not path.is_file():
            return {"input": str(path), "status": "failed", "error": "file not found"}
        try:
            return run_cli(
                cli_bin,
                path,
                config_json,
                args.timeout_seconds,
                args,
                stderr_prefix=path.name,
            )
        except JobClientError as exc:
            return {"input": str(path), "status": "failed", "error": str(exc)}

    old_int = signal.signal(signal.SIGINT, handle_signal)
    old_term = signal.signal(signal.SIGTERM, handle_signal)
    try:
        with concurrent.futures.ThreadPoolExecutor(max_workers=max_workers) as executor:
            future_to_index = {
                executor.submit(run_index, index, path): index for index, path in enumerate(files)
            }
            for future in concurrent.futures.as_completed(future_to_index):
                index = future_to_index[future]
                result = future.result()
                results[index] = result
                if args.fail_fast and result.get("status") == "failed":
                    for pending in future_to_index:
                        pending.cancel()
                    break
    except KeyboardInterrupt:
        terminate_children()
        final_results = [result for result in results if result is not None] + skipped
        final_results.append({"status": "failed", "error": "batch interrupted"})
        totals = {
            "ok": sum(1 for result in final_results if result.get("status") == "completed"),
            "failed": sum(1 for result in final_results if result.get("status") == "failed"),
            "skipped_cached": sum(1 for result in final_results if result.get("cached")),
            "skipped_unsupported": sum(
                1 for result in final_results if result.get("reason") == "unsupported"
            ),
        }
        return {
            "batch": True,
            "backend": "cli",
            "interrupted": interrupted,
            "totals": totals,
            "wall_clock_ms": int((time.time() - started) * 1000),
            "results": final_results,
        }
    finally:
        signal.signal(signal.SIGINT, old_int)
        signal.signal(signal.SIGTERM, old_term)

    final_results = [result for result in results if result is not None] + skipped
    totals = {
        "ok": sum(1 for result in final_results if result.get("status") == "completed"),
        "failed": sum(1 for result in final_results if result.get("status") == "failed"),
        "skipped_cached": sum(1 for result in final_results if result.get("cached")),
        "skipped_unsupported": sum(
            1 for result in final_results if result.get("reason") == "unsupported"
        ),
    }
    return {
        "batch": True,
        "backend": "cli",
        "totals": totals,
        "wall_clock_ms": int((time.time() - started) * 1000),
        "results": final_results,
    }


PROGRESS_RE = re.compile(
    r"^\[(?P<stage_index>\d+)/(?P<total_stages>\d+)\]\s+"
    r"(?P<stage>\w+)(?:\s+(?P<page>\d+)/(?P<total_pages>\d+))?\s+"
    r"(?P<progress>\d+)%\s+(?P<message>.*)$"
)


def cli_jobs_home() -> Path:
    return summarizer_home() / "cli-jobs"


def write_json_atomic(path: Path, data: dict[str, Any]) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    with tempfile.NamedTemporaryFile("w", encoding="utf-8", dir=path.parent, delete=False) as handle:
        json.dump(data, handle, indent=2, sort_keys=True)
        handle.write("\n")
        temp_name = handle.name
    os.replace(temp_name, path)


def read_json(path: Path) -> dict[str, Any]:
    return json.loads(path.read_text(encoding="utf-8"))


def detached_paths(run_id: str) -> tuple[Path, Path, Path]:
    root = cli_jobs_home() / run_id
    return root / "status.json", root / "runner.log", root / "manifest.json"


def submit_detached(args: argparse.Namespace) -> dict[str, Any]:
    run_id = str(uuid.uuid4())
    status_path, log_path, manifest_path = detached_paths(run_id)
    status_path.parent.mkdir(parents=True, exist_ok=True)
    original = [arg for arg in sys.argv[1:] if arg != "--detach"]
    command = [sys.executable, str(Path(__file__).resolve()), "--_runner", run_id, *original]
    log_handle = log_path.open("w", encoding="utf-8")
    process = subprocess.Popen(
        command,
        stdin=subprocess.DEVNULL,
        stdout=log_handle,
        stderr=log_handle,
        text=True,
        start_new_session=True,
    )
    pgid = os.getpgid(process.pid)
    status = {
        "run_id": run_id,
        "status": "queued",
        "pid": process.pid,
        "pgid": pgid,
        "submitted_at": datetime.now(timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ"),
        "status_path": str(status_path),
        "log_path": str(log_path),
        "manifest_path": str(manifest_path),
    }
    write_json_atomic(status_path, status)
    return status


def run_detached_runner(args: argparse.Namespace) -> int:
    run_id = args._runner
    status_path, _log_path, manifest_path = detached_paths(run_id)
    files, skipped, is_batch = collect_inputs(args)
    if is_batch or skipped or len(files) != 1:
        write_json_atomic(status_path, {"run_id": run_id, "status": "failed", "error": "--detach supports exactly one file"})
        return 1
    cli_bin = resolve_cli_bin(args.cli_bin)
    if cli_bin is None:
        write_json_atomic(status_path, {"run_id": run_id, "status": "failed", "error": "summarizer-cli not found"})
        return 1
    config_json = build_config_json(args)

    def update_from_progress(line: str) -> None:
        match = PROGRESS_RE.match(line)
        if not match:
            return
        payload: dict[str, Any] = {
            "run_id": run_id,
            "status": "running",
            "pid": os.getpid(),
            "pgid": os.getpgrp(),
            "stage": match.group("stage"),
            "stage_index": int(match.group("stage_index")),
            "total_stages": int(match.group("total_stages")),
            "progress": int(match.group("progress")),
            "message": match.group("message"),
            "updated_at": datetime.now(timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ"),
        }
        if match.group("page"):
            payload["page"] = int(match.group("page"))
            payload["total_pages"] = int(match.group("total_pages"))
        write_json_atomic(status_path, payload)

    write_json_atomic(status_path, {
        "run_id": run_id,
        "status": "running",
        "pid": os.getpid(),
        "pgid": os.getpgrp(),
        "updated_at": datetime.now(timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ"),
    })
    try:
        manifest = run_cli(
            cli_bin,
            files[0],
            config_json,
            args.timeout_seconds,
            args,
            status_callback=update_from_progress,
            detached=True,
            job_id=run_id,
        )
        manifest["detached"] = True
        write_json_atomic(manifest_path, manifest)
        write_json_atomic(status_path, {
            "run_id": run_id,
            "status": "completed",
            "pid": os.getpid(),
            "pgid": os.getpgrp(),
            "manifest_path": str(manifest_path),
            "updated_at": datetime.now(timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ"),
        })
        return 0
    except JobClientError as exc:
        manifest = {"backend": "cli", "detached": True, "run_id": run_id, "status": "failed", "error": str(exc)}
        write_json_atomic(manifest_path, manifest)
        write_json_atomic(status_path, {
            "run_id": run_id,
            "status": "failed",
            "pid": os.getpid(),
            "pgid": os.getpgrp(),
            "error": str(exc),
            "manifest_path": str(manifest_path),
            "updated_at": datetime.now(timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ"),
        })
        return 1


def status_for(run_id: str) -> dict[str, Any]:
    status_path, _log_path, manifest_path = detached_paths(run_id)
    if not status_path.is_file():
        raise JobClientError(f"No detached job status found for {run_id}")
    status = read_json(status_path)
    if status.get("status") not in TERMINAL_STATUSES:
        pid = status.get("pid")
        if isinstance(pid, int):
            try:
                os.kill(pid, 0)
            except OSError:
                status["status"] = "failed"
                status["error"] = "runner died"
                write_json_atomic(status_path, status)
    if status.get("status") in TERMINAL_STATUSES and manifest_path.is_file():
        status["manifest"] = read_json(manifest_path)
    return status


def status_all() -> list[dict[str, Any]]:
    root = cli_jobs_home()
    if not root.is_dir():
        return []
    statuses = []
    for child in sorted(root.iterdir(), key=lambda path: path.stat().st_mtime, reverse=True):
        if child.is_dir():
            try:
                statuses.append(status_for(child.name))
            except JobClientError:
                continue
    live = [status for status in statuses if status.get("status") not in TERMINAL_STATUSES]
    terminal = [status for status in statuses if status.get("status") in TERMINAL_STATUSES][:5]
    return live + terminal


def wait_for_detached(run_id: str, timeout_seconds: float) -> dict[str, Any]:
    started = time.time()
    while True:
        status = status_for(run_id)
        if status.get("status") in TERMINAL_STATUSES:
            manifest = status.get("manifest")
            return manifest if isinstance(manifest, dict) else status
        if time.time() - started > timeout_seconds:
            raise JobClientError(f"Timed out waiting for {run_id}: {status}")
        time.sleep(1)


def cancel_detached(run_id: str) -> dict[str, Any]:
    status_path, _log_path, _manifest_path = detached_paths(run_id)
    status = status_for(run_id)
    pgid = status.get("pgid")
    if status.get("status") in TERMINAL_STATUSES:
        return status
    if not isinstance(pgid, int):
        raise JobClientError(f"Detached job {run_id} has no process group.")
    try:
        os.killpg(pgid, signal.SIGTERM)
    except ProcessLookupError:
        pass
    deadline = time.time() + 10
    while time.time() < deadline:
        try:
            os.killpg(pgid, 0)
        except ProcessLookupError:
            break
        time.sleep(0.5)
    else:
        try:
            os.killpg(pgid, signal.SIGKILL)
        except ProcessLookupError:
            pass
    status["status"] = "canceled"
    status["updated_at"] = datetime.now(timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ")
    write_json_atomic(status_path, status)
    return status


def convert_manifest_to_okf(manifest: dict[str, Any], args: argparse.Namespace) -> None:
    if not args.okf:
        return
    output_path = manifest.get("output_json_path")
    if manifest.get("status") != "completed" or not isinstance(output_path, str):
        return
    argv = ["--input", output_path, "--granularity", args.okf_granularity]
    if args.okf_out:
        argv += ["--out", args.okf_out]
    try:
        okf_manifest = to_okf.run(argv)
    except Exception as exc:
        manifest["okf_error"] = str(exc)
    else:
        manifest.update({
            key: value
            for key, value in okf_manifest.items()
            if key in {"bundle_dir", "file_written", "format", "okf_version"}
        })


def convert_okf_outputs(manifest: dict[str, Any], args: argparse.Namespace) -> None:
    if not args.okf:
        return
    if manifest.get("batch"):
        for result in manifest.get("results", []):
            if isinstance(result, dict):
                convert_manifest_to_okf(result, args)
    else:
        convert_manifest_to_okf(manifest, args)


def run_app(args: argparse.Namespace, file_path: Path, config_json: str | None) -> dict[str, Any]:
    app_bin = resolve_app_bin(args.app_bin)
    home = summarizer_home()

    before = snapshot_job_ids(home)
    eprint(f"app_bin={app_bin}")
    eprint(f"summarizer_home={home}")
    forward_enqueue(app_bin, file_path, config_json, args.enqueue_timeout_seconds)

    job = wait_for_new_job(
        home, before, file_path.name, args.poll_interval, args.timeout_seconds
    )
    job_id = job["job_id"]

    manifest: dict[str, Any] = {
        "backend": "app",
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
    return manifest


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
        "page_range": args.pages,
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
    parser.add_argument("--dir", help="Process supported documents in a folder")
    parser.add_argument("--files", nargs="+", action="append", help="Process an explicit list of files; repeatable")
    parser.add_argument("--glob", action="append", help="Glob pattern for --dir (repeatable; default all files)")
    parser.add_argument("--recursive", action="store_true", help="Recurse when used with --dir")
    parser.add_argument("--parallel", type=int, default=2, help="Batch parallelism, capped at 4")
    parser.add_argument("--fail-fast", action="store_true", help="Stop scheduling batch work after the first failure")
    parser.add_argument("--favorite", help="Apply a saved favorite preset by name or number (see --list-favorites)")
    parser.add_argument("--require-favorite", action="store_true", help="Fail unless --favorite is supplied (for automation)")
    parser.add_argument("--list-favorites", action="store_true", help="Print the saved favorite presets as JSON and exit")
    parser.add_argument("--list-runs", action="store_true", help="Print CLI catalog runs as JSON and exit")
    parser.add_argument("--limit", type=int, default=None, help="Limit --list-runs results")
    parser.add_argument("--for", dest="for_path", help="Filter --list-runs by input path")
    parser.add_argument("--force", action="store_true", help="Bypass cache and run even if an unchanged result exists")
    parser.add_argument("--no-cache", action="store_true", help="Bypass cache and skip catalog append")
    parser.add_argument("--detach", action="store_true", help="Run one CLI job in the background and return a run handle")
    parser.add_argument("--status", help="Read detached job status by run id, or 'all'")
    parser.add_argument("--wait", help="Wait for a detached run id and print its terminal manifest")
    parser.add_argument("--cancel", help="Cancel a detached run id")
    parser.add_argument("--_runner", help=argparse.SUPPRESS)
    parser.add_argument("--okf", action="store_true", help="Convert successful output JSON to OKF after processing")
    parser.add_argument("--okf-out", help="Output directory/path for --okf")
    parser.add_argument("--okf-granularity", choices=["pages", "single"], default="pages", help="OKF output granularity")
    parser.add_argument("--app-bin", help="Path to the desktop app binary (default: SUMMARIZER_APP_BIN, repo target, or the macOS bundle)")
    parser.add_argument("--cli-bin", help="Path to the headless summarizer-cli binary (default: SUMMARIZER_CLI_BIN, repo target, or PATH)")
    parser.add_argument("--backend", choices=["auto", "cli", "app"], default="auto", help="Execution backend: auto prefers CLI, cli forces headless, app forces desktop enqueue")
    parser.add_argument("--config-json", help="JSON PipelineConfig override; omit to use the app's default + saved settings")
    parser.add_argument("--settings", help="Path to settings.json for the headless CLI")
    parser.add_argument("--env-providers", action="store_true", help="Use provider settings from environment variables")
    parser.add_argument("--pdfium", help="Path to the PDFium library for PDF/PPTX inputs")
    parser.add_argument("--doctor", action="store_true", help="Validate skill, CLI, PDFium, settings, and providers without running the pipeline")
    parser.add_argument("--estimate", action="store_true", help="Dry-run the document structure and provider-call plan without model calls")
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
    extraction.add_argument(
        "--pages", default=None, help="Only process selected pages/chunks, e.g. 1-3,8,10",
    )
    extraction.add_argument(
        "--sample", type=int, default=None, help="Estimate page count, then process a front/middle/back sample of N pages",
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

    if args.list_runs:
        print(json.dumps(find_runs(args.for_path, args.limit), indent=2, sort_keys=True))
        return 0

    if args.status:
        report = status_all() if args.status == "all" else status_for(args.status)
        print(json.dumps(report, indent=2, sort_keys=True))
        return 0

    if args.wait:
        report = wait_for_detached(args.wait, args.timeout_seconds)
        print(json.dumps(report, indent=2, sort_keys=True))
        return 0 if report.get("status") == "completed" else 1

    if args.cancel:
        report = cancel_detached(args.cancel)
        print(json.dumps(report, indent=2, sort_keys=True))
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

    if args.require_favorite and not args.favorite:
        # Usage error (exit 2): automation callers must name their favorite explicitly.
        eprint("ERROR: --require-favorite was set but no --favorite was provided. Run --list-favorites.")
        return 2

    if args.pages and args.sample is not None:
        raise JobClientError("--pages and --sample are mutually exclusive.")

    if args._runner:
        return run_detached_runner(args)

    if args.doctor:
        config_json = build_config_json(args)
        manifest = run_doctor(args, config_json)
        print(json.dumps(manifest, indent=2, sort_keys=True))
        return 0 if manifest.get("ok") else 1

    if args.detach:
        if args.backend == "app":
            raise JobClientError("--detach requires the CLI backend; --backend app is not supported.")
        files, skipped, is_batch = collect_inputs(args)
        if is_batch or skipped or len(files) != 1:
            raise JobClientError("--detach supports exactly one --file input.")
        manifest = submit_detached(args)
        print(json.dumps(manifest, indent=2, sort_keys=True))
        return 0

    files, skipped, is_batch = collect_inputs(args)

    if not files and not skipped:
        raise JobClientError("--file is required (unless --list-favorites or --doctor).")

    if not is_batch:
        file_path = files[0]
        if not file_path.is_file():
            raise JobClientError(f"File not found: {file_path}")

    config_json = build_config_json(args)

    cli_bin = resolve_cli_bin(args.cli_bin)
    if is_batch:
        if args.sample is not None:
            raise JobClientError("--sample is only supported for single-file runs.")
        if args.backend == "app":
            raise JobClientError("Batch processing requires the CLI backend; --backend app is not supported.")
        if cli_bin is None:
            raise JobClientError("Could not locate summarizer-cli. Set SUMMARIZER_CLI_BIN or pass --cli-bin.")
        eprint(f"cli_bin={cli_bin}")
        manifest = process_batch(cli_bin, files, skipped, config_json, args)
    elif args.estimate:
        if cli_bin is None:
            raise JobClientError("Could not locate summarizer-cli. Set SUMMARIZER_CLI_BIN or pass --cli-bin.")
        eprint(f"cli_bin={cli_bin}")
        manifest = run_estimate(cli_bin, file_path, config_json, args.timeout_seconds, args)
    elif args.backend == "cli":
        if cli_bin is None:
            raise JobClientError("Could not locate summarizer-cli. Set SUMMARIZER_CLI_BIN or pass --cli-bin.")
        eprint(f"cli_bin={cli_bin}")
        config_json = apply_sample_range(cli_bin, file_path, config_json, args)
        manifest = run_cli(cli_bin, file_path, config_json, args.timeout_seconds, args)
    elif args.backend == "auto" and cli_bin is not None:
        eprint(f"cli_bin={cli_bin}")
        config_json = apply_sample_range(cli_bin, file_path, config_json, args)
        manifest = run_cli(cli_bin, file_path, config_json, args.timeout_seconds, args)
    else:
        if args.sample is not None:
            raise JobClientError("--sample requires the CLI backend.")
        if args.backend == "auto":
            eprint("summarizer-cli not found; falling back to desktop app enqueue path")
        manifest = run_app(args, file_path, config_json)

    convert_okf_outputs(manifest, args)
    print(json.dumps(manifest, indent=2, sort_keys=True))
    if manifest.get("batch") and manifest.get("totals", {}).get("failed", 0):
        return 1
    return 0


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except JobClientError as exc:
        eprint(f"ERROR: {exc}")
        raise SystemExit(1)
