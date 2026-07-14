#!/usr/bin/env python3
"""Build a deterministic corpus brief from completed summarizer CLI runs."""

from __future__ import annotations

import argparse
import fnmatch
import json
import subprocess
import sys
from pathlib import Path
from typing import Any

from _catalog import find_runs
from _common import SkillError, first_description, load_output_from_path


def completed_runs() -> list[dict[str, Any]]:
    return [
        run
        for run in find_runs()
        if run.get("status") == "completed"
        and isinstance(run.get("output_json_path"), str)
        and Path(run["output_json_path"]).is_file()
    ]


def select_runs(args: argparse.Namespace) -> list[dict[str, Any]]:
    runs = completed_runs()
    if args.runs:
        wanted = set(args.runs)
        runs = [run for run in runs if run.get("run_id") in wanted]
    elif args.latest:
        runs = runs[: args.latest]
    elif args.for_glob:
        runs = [
            run
            for run in runs
            if fnmatch.fnmatch(Path(str(run.get("input_path", ""))).name, args.for_glob)
        ]
    if len(runs) < 2:
        raise SkillError(f"Need at least 2 completed runs; found {len(runs)}.")
    return runs


def load_run_output(run: dict[str, Any]) -> tuple[dict[str, Any], Path]:
    return load_output_from_path(Path(str(run["output_json_path"])))


def deterministic_brief(runs: list[dict[str, Any]]) -> str:
    loaded = [(run, *load_run_output(run)) for run in runs]
    lines = ["# Corpus Brief", "", "## Inventory", ""]
    lines.append("| Document | Pages | Run Timestamp |")
    lines.append("|---|---:|---|")
    for run, output, _path in loaded:
        doc = output.get("document") or {}
        lines.append(
            f"| {doc.get('filename', Path(str(run.get('input_path', 'document'))).name)} "
            f"| {doc.get('total_pages', len(output.get('pages') or []))} "
            f"| {run.get('timestamp', '')} |"
        )

    topic_counts: dict[str, dict[str, int]] = {}
    for _run, output, _path in loaded:
        filename = (output.get("document") or {}).get("filename", "document")
        for page in output.get("pages") or []:
            if not isinstance(page, dict):
                continue
            for topic in page.get("summary_topics") or []:
                key = str(topic).strip()
                if not key:
                    continue
                canonical = key.lower()
                topic_counts.setdefault(canonical, {"label": key, "total": 0})
                topic_counts[canonical]["total"] += 1
                topic_counts[canonical][filename] = topic_counts[canonical].get(filename, 0) + 1

    lines += ["", "## Topics", ""]
    if topic_counts:
        lines.append("| Topic | Total Mentions |")
        lines.append("|---|---:|")
        for item in sorted(topic_counts.values(), key=lambda value: (-int(value["total"]), str(value["label"]).lower())):
            lines.append(f"| {item['label']} | {item['total']} |")
    else:
        lines.append("_No summary topics found._")

    lines += ["", "## Document Digests", ""]
    for _run, output, _path in loaded:
        doc = output.get("document") or {}
        pages = [page for page in output.get("pages") or [] if isinstance(page, dict)]
        summarized = [first_description(page) for page in pages if first_description(page)]
        lines.append(f"### {doc.get('filename', 'document')}")
        if not summarized:
            lines.append("_No summaries available._")
        else:
            for note in summarized[:5]:
                lines.append(f"- {note}")
        lines.append("")
    return "\n".join(lines).rstrip() + "\n"


def maybe_append_llm(brief: str, args: argparse.Namespace) -> tuple[str, str | None]:
    if not args.llm:
        return brief, None
    executable = args.llm_executable or "codex"
    prompt = (
        "Synthesize cross-document themes, agreements, contradictions, and gaps "
        "from this deterministic corpus brief. Use only the provided notes/topics.\n\n"
        + brief
    )
    try:
        completed = subprocess.run(
            [executable],
            input=prompt,
            capture_output=True,
            text=True,
            timeout=args.timeout_seconds,
            check=False,
        )
    except (OSError, subprocess.TimeoutExpired) as exc:
        print(f"WARNING: LLM synthesis unavailable: {exc}", file=sys.stderr)
        return brief, None
    if completed.returncode != 0:
        print(f"WARNING: LLM synthesis exited {completed.returncode}: {completed.stderr}", file=sys.stderr)
        return brief, None
    return brief + "\n## LLM Synthesis\n\n" + completed.stdout.strip() + "\n", executable


def build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(description="Create a corpus brief from CLI catalog runs.")
    group = parser.add_mutually_exclusive_group(required=True)
    group.add_argument("--runs", nargs="+", help="Specific catalog run ids")
    group.add_argument("--latest", type=int, help="Use the latest N completed runs")
    group.add_argument("--for", dest="for_glob", help="Glob against input basenames")
    parser.add_argument("--out", help="Output markdown path (default: ./corpus-brief.md)")
    parser.add_argument("--llm", action="store_true", help="Append a best-effort one-shot CLI LLM synthesis")
    parser.add_argument("--llm-executable", help="Executable for --llm (default: codex)")
    parser.add_argument("--timeout-seconds", type=float, default=120.0)
    return parser


def main() -> int:
    args = build_parser().parse_args()
    runs = select_runs(args)
    brief, provider = maybe_append_llm(deterministic_brief(runs), args)
    out = Path(args.out or "corpus-brief.md").expanduser()
    out.parent.mkdir(parents=True, exist_ok=True)
    out.write_text(brief, encoding="utf-8")
    print(json.dumps({
        "brief_path": str(out),
        "runs": [run.get("run_id") for run in runs],
        "provider": provider,
    }, indent=2, sort_keys=True))
    return 0


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except SkillError as exc:
        print(f"ERROR: {exc}", file=sys.stderr)
        raise SystemExit(2)
