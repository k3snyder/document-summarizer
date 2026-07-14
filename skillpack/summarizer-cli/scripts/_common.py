#!/usr/bin/env python3
"""Shared helpers for Document Summarizer skill scripts."""

from __future__ import annotations

import json
import os
import re
import sys
from pathlib import Path
from typing import Any


class SkillError(RuntimeError):
    """Expected client-side failure."""


def eprint(message: str) -> None:
    print(message, file=sys.stderr)


def summarizer_home() -> Path:
    home = os.environ.get("SUMMARIZER_HOME") or os.environ.get("HOME")
    if not home:
        raise SkillError("Could not determine home directory (set HOME or SUMMARIZER_HOME).")
    return Path(home).expanduser() / ".summarizer"


def load_output_from_path(path: Path) -> tuple[dict[str, Any], Path]:
    if not path.is_file():
        raise SkillError(f"output.json not found: {path}")
    try:
        data = json.loads(path.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError) as exc:
        raise SkillError(f"Could not parse {path}: {exc}") from exc
    if not isinstance(data, dict) or "document" not in data:
        raise SkillError(f"{path} is not a summarizer output (missing 'document').")
    return data, path


def load_output(args: Any) -> tuple[dict[str, Any], Path]:
    if getattr(args, "run", None):
        from _catalog import find_runs

        selector = args.run
        runs = find_runs(limit=1) if selector == "latest" else [
            entry for entry in find_runs() if entry.get("run_id") == selector
        ]
        if not runs:
            raise SkillError(f"No catalog run found for {selector}.")
        output = runs[0].get("output_json_path")
        if not isinstance(output, str):
            raise SkillError(f"Catalog run {selector} has no output_json_path.")
        return load_output_from_path(Path(output))
    if getattr(args, "input", None):
        return load_output_from_path(Path(args.input).expanduser())
    if getattr(args, "job_id", None):
        return load_output_from_path(summarizer_home() / "jobs" / args.job_id / "output.json")

    history_path = summarizer_home() / "history.json"
    try:
        history = json.loads(history_path.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError) as exc:
        raise SkillError(f"Could not read history.json: {exc}") from exc
    completed = [
        job
        for job in history
        if isinstance(job, dict) and job.get("status") == "completed" and job.get("job_id")
    ]
    if not completed:
        raise SkillError("No completed jobs found in history.json.")
    return load_output_from_path(summarizer_home() / "jobs" / completed[0]["job_id"] / "output.json")


def visual_text(page: dict[str, Any]) -> str:
    parts = [
        page.get("image_text"),
        page.get("image_text_1"),
        page.get("image_text_2"),
        page.get("image_text_3"),
    ]
    return "\n\n".join(part.strip() for part in parts if isinstance(part, str) and part.strip())


def first_description(page: dict[str, Any]) -> str:
    notes = page.get("summary_notes") or []
    if notes:
        return str(notes[0]).strip()
    text = (page.get("text") or "").strip()
    if text:
        flat = " ".join(text.split())
        return (flat[:117] + "...") if len(flat) > 120 else flat
    return visual_text(page)[:120].strip()


def page_number(page: dict[str, Any], idx: int) -> int:
    value = page.get("page_number")
    return value if isinstance(value, int) and value > 0 else idx + 1


def parse_page_range(spec: str) -> set[int]:
    selected: set[int] = set()
    for segment in spec.split(","):
        segment = segment.strip()
        if not segment:
            raise SkillError("Page range contains an empty segment.")
        if "-" in segment:
            raw_start, raw_end = segment.split("-", 1)
            if not raw_start.isdigit() or not raw_end.isdigit():
                raise SkillError(f"Invalid page range segment: {segment}")
            start, end = int(raw_start), int(raw_end)
            if start <= 0 or end <= 0 or start > end:
                raise SkillError(f"Invalid page range segment: {segment}")
            selected.update(range(start, end + 1))
        else:
            if not segment.isdigit() or int(segment) <= 0:
                raise SkillError(f"Invalid page number: {segment}")
            selected.add(int(segment))
    return selected


def strip_images(value: Any) -> Any:
    if isinstance(value, dict):
        cleaned = {}
        for key, item in value.items():
            if key == "image_base64":
                continue
            if key == "base64" and "id" in value:
                continue
            cleaned[key] = strip_images(item)
        return cleaned
    if isinstance(value, list):
        return [strip_images(item) for item in value]
    return value


def json_dump(data: Any) -> None:
    print(json.dumps(data, indent=2, sort_keys=True, ensure_ascii=True))


def regex_snippets(pattern: re.Pattern[str], text: str, context: int = 80) -> list[str]:
    snippets: list[str] = []
    flat = " ".join(text.split())
    for match in pattern.finditer(flat):
        start = max(match.start() - context, 0)
        end = min(match.end() + context, len(flat))
        snippets.append(flat[start:end])
    return snippets
