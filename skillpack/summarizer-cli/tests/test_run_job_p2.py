#!/usr/bin/env python3

from __future__ import annotations

import json
import os
import subprocess
import sys
import tempfile
from pathlib import Path


ROOT = Path(__file__).resolve().parents[1]
RUN_JOB = ROOT / "scripts" / "run_job.py"
CONFIG = '{"extract_only":true,"vision_mode":"none","run_summarization":false}'


def run_job(args: list[str], home: Path) -> subprocess.CompletedProcess[str]:
    env = os.environ.copy()
    env["SUMMARIZER_HOME"] = str(home)
    return subprocess.run(
        [sys.executable, str(RUN_JOB), *args],
        capture_output=True,
        text=True,
        check=False,
        env=env,
    )


def test_cache_hit_and_list_runs() -> None:
    with tempfile.TemporaryDirectory() as temp:
        root = Path(temp)
        home = root / "home"
        input_path = root / "sample.txt"
        input_path.write_text("Alpha\n", encoding="utf-8")
        args = [
            "--file",
            str(input_path),
            "--backend",
            "cli",
            "--env-providers",
            "--config-json",
            CONFIG,
        ]

        first = run_job(args, home)
        assert first.returncode == 0, first.stderr
        first_manifest = json.loads(first.stdout)
        assert first_manifest["status"] == "completed"

        second = run_job(args, home)
        assert second.returncode == 0, second.stderr
        second_manifest = json.loads(second.stdout)
        assert second_manifest["cached"] is True
        assert second_manifest["output_json_path"] == first_manifest["output_json_path"]

        listed = run_job(["--list-runs", "--for", str(input_path)], home)
        assert listed.returncode == 0, listed.stderr
        runs = json.loads(listed.stdout)
        assert len(runs) == 1
        assert runs[0]["status"] == "completed"


def test_batch_processes_directory() -> None:
    with tempfile.TemporaryDirectory() as temp:
        root = Path(temp)
        home = root / "home"
        docs = root / "docs"
        docs.mkdir()
        for index in range(3):
            (docs / f"doc{index}.txt").write_text(f"Doc {index}\n", encoding="utf-8")
        (docs / "skip.bin").write_bytes(b"skip")

        completed = run_job(
            [
                "--dir",
                str(docs),
                "--backend",
                "cli",
                "--env-providers",
                "--config-json",
                CONFIG,
                "--parallel",
                "2",
            ],
            home,
        )
        assert completed.returncode == 0, completed.stderr
        manifest = json.loads(completed.stdout)
        assert manifest["batch"] is True
        assert manifest["totals"]["ok"] == 3
        assert manifest["totals"]["skipped_unsupported"] == 1


def test_detach_wait_completes() -> None:
    with tempfile.TemporaryDirectory() as temp:
        root = Path(temp)
        home = root / "home"
        input_path = root / "sample.txt"
        input_path.write_text("Alpha\n", encoding="utf-8")

        submitted = run_job(
            [
                "--file",
                str(input_path),
                "--detach",
                "--env-providers",
                "--config-json",
                CONFIG,
            ],
            home,
        )
        assert submitted.returncode == 0, submitted.stderr
        submit_manifest = json.loads(submitted.stdout)
        run_id = submit_manifest["run_id"]

        waited = run_job(["--wait", run_id, "--timeout-seconds", "30"], home)
        assert waited.returncode == 0, waited.stderr
        manifest = json.loads(waited.stdout)
        assert manifest["status"] == "completed"
        assert manifest["detached"] is True


def test_pages_sample_and_okf_passthrough() -> None:
    with tempfile.TemporaryDirectory() as temp:
        root = Path(temp)
        home = root / "home"
        input_path = root / "sample.txt"
        input_path.write_text("Alpha. Beta. Gamma. Delta.", encoding="utf-8")

        completed = run_job(
            [
                "--file",
                str(input_path),
                "--backend",
                "cli",
                "--env-providers",
                "--config-json",
                '{"extract_only":true,"vision_mode":"none","run_summarization":false,"chunk_size":7,"chunk_overlap":0}',
                "--pages",
                "2-3",
                "--okf",
                "--okf-granularity",
                "single",
            ],
            home,
        )
        assert completed.returncode == 0, completed.stderr
        manifest = json.loads(completed.stdout)
        assert manifest["status"] == "completed"
        assert manifest["format"] == "okf-single"
        assert Path(manifest["file_written"]).is_file()
        output = json.loads(Path(manifest["output_json_path"]).read_text(encoding="utf-8"))
        page_numbers = [page["page_number"] for page in output["pages"]]
        assert page_numbers == [2, 3], page_numbers
