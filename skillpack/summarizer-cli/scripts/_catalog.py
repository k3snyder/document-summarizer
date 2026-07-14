#!/usr/bin/env python3
"""Append-only catalog/cache for headless summarizer-cli runs."""

from __future__ import annotations

import hashlib
import json
import os
import sys
from pathlib import Path
from typing import Any, Iterator

from _common import summarizer_home


CATALOG_VERSION = 1
CATALOG = summarizer_home() / "cli-runs.jsonl"


def append_run(entry: dict[str, Any]) -> None:
    CATALOG.parent.mkdir(parents=True, exist_ok=True)
    payload = dict(entry)
    payload.setdefault("catalog_version", CATALOG_VERSION)
    line = json.dumps(payload, separators=(",", ":"), sort_keys=True, ensure_ascii=True) + "\n"
    flags = os.O_APPEND | os.O_CREAT | os.O_WRONLY
    fd = os.open(CATALOG, flags, 0o600)
    try:
        os.write(fd, line.encode("utf-8"))
    finally:
        os.close(fd)


def iter_runs() -> Iterator[dict[str, Any]]:
    if not CATALOG.is_file():
        return
    with CATALOG.open(encoding="utf-8") as handle:
        for line_no, line in enumerate(handle, start=1):
            line = line.strip()
            if not line:
                continue
            try:
                entry = json.loads(line)
            except json.JSONDecodeError:
                print(f"WARNING: ignoring corrupt catalog line {line_no}", file=sys.stderr)
                continue
            if isinstance(entry, dict):
                yield entry


def find_runs(input_path: str | None = None, limit: int | None = None) -> list[dict[str, Any]]:
    resolved = str(Path(input_path).expanduser().resolve()) if input_path else None
    matches = [
        entry
        for entry in iter_runs()
        if resolved is None or entry.get("input_path") == resolved
    ]
    matches.reverse()
    return matches[:limit] if limit is not None else matches


def find_cached(input_sha256: str, config_hash: str) -> dict[str, Any] | None:
    for entry in find_runs():
        if entry.get("status") != "completed":
            continue
        if entry.get("input_sha256") != input_sha256:
            continue
        if entry.get("config_hash") != config_hash:
            continue
        output = entry.get("output_json_path")
        if isinstance(output, str) and Path(output).is_file():
            return entry
    return None


def file_sha256(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as handle:
        for chunk in iter(lambda: handle.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def config_hash(config_json: str) -> str:
    return hashlib.sha256(config_json.encode("utf-8")).hexdigest()
