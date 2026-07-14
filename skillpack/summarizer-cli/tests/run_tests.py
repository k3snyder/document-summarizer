#!/usr/bin/env python3
"""Tiny stdlib-only test runner for the summarizer-cli skillpack."""

from __future__ import annotations

import importlib.util
import sys
import traceback
from pathlib import Path


class Skip(RuntimeError):
    pass


def main() -> int:
    root = Path(__file__).resolve().parent
    passed = failed = skipped = 0
    for path in sorted(root.glob("test_*.py")):
        spec = importlib.util.spec_from_file_location(path.stem, path)
        if spec is None or spec.loader is None:
            print(f"FAIL {path.name}: could not load module")
            failed += 1
            continue
        module = importlib.util.module_from_spec(spec)
        module.Skip = Skip
        sys.modules[path.stem] = module
        spec.loader.exec_module(module)
        for name in sorted(dir(module)):
            if not name.startswith("test_"):
                continue
            test = getattr(module, name)
            if not callable(test):
                continue
            label = f"{path.name}::{name}"
            try:
                test()
            except Skip as exc:
                print(f"SKIP {label}: {exc}")
                skipped += 1
            except Exception:
                print(f"FAIL {label}")
                traceback.print_exc()
                failed += 1
            else:
                print(f"PASS {label}")
                passed += 1
    print(f"passed={passed} skipped={skipped} failed={failed}")
    return 1 if failed else 0


if __name__ == "__main__":
    raise SystemExit(main())
