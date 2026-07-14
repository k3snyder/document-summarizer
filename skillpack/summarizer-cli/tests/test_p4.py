#!/usr/bin/env python3

from __future__ import annotations

import json
import os
import shutil
import subprocess
import sys
import tempfile
from pathlib import Path


ROOT = Path(__file__).resolve().parents[1]
RUN_JOB = ROOT / "scripts" / "run_job.py"
SYNTHESIZE = ROOT / "scripts" / "synthesize.py"
FIXTURE = ROOT / "tests" / "fixtures" / "sample_output.json"
CONFIG = '{"extract_only":true,"vision_mode":"none","run_summarization":false}'


def env_for(home: Path) -> dict[str, str]:
    env = os.environ.copy()
    env["SUMMARIZER_HOME"] = str(home)
    return env


def test_require_favorite_guard() -> None:
    with tempfile.TemporaryDirectory() as temp:
        root = Path(temp)
        input_path = root / "sample.txt"
        input_path.write_text("Alpha\n", encoding="utf-8")
        completed = subprocess.run(
            [
                sys.executable,
                str(RUN_JOB),
                "--file",
                str(input_path),
                "--require-favorite",
                "--env-providers",
                "--config-json",
                CONFIG,
            ],
            capture_output=True,
            text=True,
            check=False,
            env=env_for(root / "home"),
        )
        assert completed.returncode == 2
        assert "--require-favorite" in completed.stderr
        assert "--list-favorites" in completed.stderr


def test_synthesize_latest_two() -> None:
    with tempfile.TemporaryDirectory() as temp:
        root = Path(temp)
        home = root / "home"
        outputs = []
        for index in range(2):
            path = root / f"sample{index}_output.json"
            shutil.copy(FIXTURE, path)
            outputs.append(path)
        catalog = home / ".summarizer" / "cli-runs.jsonl"
        catalog.parent.mkdir(parents=True)
        for index, path in enumerate(outputs):
            entry = {
                "catalog_version": 1,
                "run_id": f"run{index}",
                "timestamp": f"2026-07-02T00:00:0{index}Z",
                "input_path": str(root / f"sample{index}.txt"),
                "input_sha256": "x",
                "config_hash": "y",
                "backend": "cli",
                "output_json_path": str(path),
                "status": "completed",
            }
            with catalog.open("a", encoding="utf-8") as handle:
                handle.write(json.dumps(entry) + "\n")
        out = root / "brief.md"
        completed = subprocess.run(
            [sys.executable, str(SYNTHESIZE), "--latest", "2", "--out", str(out)],
            capture_output=True,
            text=True,
            check=False,
            env=env_for(home),
        )
        assert completed.returncode == 0, completed.stderr
        manifest = json.loads(completed.stdout)
        assert manifest["brief_path"] == str(out)
        brief = out.read_text(encoding="utf-8")
        assert "## Inventory" in brief
        assert "Revenue" in brief
