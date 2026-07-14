#!/usr/bin/env python3

from __future__ import annotations

import csv
import json
import subprocess
import sys
import tempfile
from pathlib import Path


ROOT = Path(__file__).resolve().parents[1]
FIXTURE = ROOT / "tests" / "fixtures" / "sample_output.json"
QUERY = ROOT / "scripts" / "query_result.py"
RUN_JOB = ROOT / "scripts" / "run_job.py"
IMAGE_BASE64 = "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/x8AAwMCAO+/p9sAAAAASUVORK5CYII="


def run_json(*args: str) -> dict:
    completed = subprocess.run(
        [sys.executable, str(QUERY), "--input", str(FIXTURE), *args],
        capture_output=True,
        text=True,
        check=False,
    )
    assert completed.returncode == 0, completed.stderr
    return json.loads(completed.stdout)


def test_summary_elides_images() -> None:
    completed = subprocess.run(
        [sys.executable, str(QUERY), "--input", str(FIXTURE), "--summary"],
        capture_output=True,
        text=True,
        check=False,
    )
    assert completed.returncode == 0, completed.stderr
    assert IMAGE_BASE64 not in completed.stdout
    data = json.loads(completed.stdout)
    assert data["document"]["filename"] == "sample.txt"
    assert len(data["pages"]) == 3


def test_page_fields_selector() -> None:
    data = run_json("--pages", "2-3", "--fields", "summary_topics,visual_text")
    assert [page["page_number"] for page in data["pages"]] == [2, 3]
    assert set(data["pages"][0]) == {"page_number", "summary_topics", "visual_text"}
    assert data["pages"][1]["visual_text"] == "A tiny visual marker."


def test_grep_returns_context_snippets() -> None:
    data = run_json("--grep", "revenue")
    assert [match["page_number"] for match in data["matches"]] == [1]
    assert "Revenue" in data["matches"][0]["matches"][0]


def test_topics_counts() -> None:
    data = run_json("--topics")
    topics = {item["topic"]: item["pages"] for item in data["topics"]}
    assert topics["Revenue"] == 1
    assert topics["Operations"] == 1


def test_tables_csv_round_trip() -> None:
    with tempfile.TemporaryDirectory() as temp:
        data = run_json("--tables", "--as", "csv", "--out", temp)
        files = data["tables"]["files_written"]
        assert len(files) == 1
        with Path(files[0]).open(encoding="utf-8", newline="") as handle:
            rows = list(csv.reader(handle))
        assert rows[1][1] == "1,200"
        assert rows[2][2] == "Line one\nLine two"


def test_export_pages_jsonl_and_images() -> None:
    with tempfile.TemporaryDirectory() as temp:
        data = run_json(
            "--include-images",
            "--export",
            "pages-jsonl",
            "--export",
            "images",
            "--out",
            temp,
        )
        export = data["export"]["exports"]
        pages_jsonl = Path(export["pages-jsonl"]["files_written"][0])
        assert len(pages_jsonl.read_text(encoding="utf-8").splitlines()) == 3
        image_files = export["images"]["files_written"]
        assert len(image_files) == 1
        assert Path(image_files[0]).read_bytes().startswith(b"\x89PNG")


def test_invalid_field_exits_usage() -> None:
    completed = subprocess.run(
        [sys.executable, str(QUERY), "--input", str(FIXTURE), "--fields", "nope"],
        capture_output=True,
        text=True,
        check=False,
    )
    assert completed.returncode == 2
    assert "Valid fields" in completed.stderr


def test_run_job_doctor_shape() -> None:
    completed = subprocess.run(
        [
            sys.executable,
            str(RUN_JOB),
            "--doctor",
            "--cli-bin",
            "/definitely/missing/summarizer-cli",
            "--config-json",
            '{"extract_only":true,"vision_mode":"none","run_summarization":false}',
        ],
        capture_output=True,
        text=True,
        check=False,
    )
    assert completed.returncode in (0, 1)
    data = json.loads(completed.stdout)
    assert data["doctor"] is True
    assert isinstance(data["ok"], bool)
    assert any(check["name"] == "skill:cli_bin" for check in data["checks"])
    assert any(check["name"] == "pdfium" for check in data["checks"])
