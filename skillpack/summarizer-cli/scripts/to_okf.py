#!/usr/bin/env python3
"""
Convert a Document Summarizer `output.json` into Google's Open Knowledge Format
(OKF v0.1).

OKF is "a directory of markdown files with YAML frontmatter" — a Knowledge
Bundle. By default this emits a conformant bundle:

    <out>/<slug>/
    ├── index.md            # root index (only file carrying okf_version)
    ├── log.md              # creation entry
    ├── document.md         # whole-document concept (type: Document)
    └── pages/
        ├── index.md        # links to each page concept (no frontmatter)
        ├── page-0001.md    # one concept per slide / page
        └── page-NNNN.md

`--granularity single` instead writes one self-contained markdown file with
document frontmatter and a `## Slide/Page N` section per page (a convenience
export, not a conformant bundle).

Source `output.json` is located by `--input <path>`, `--job-id <id>`
(`~/.summarizer/jobs/<id>/output.json`), or `--latest` (newest job in
`~/.summarizer/history.json`). Prints a JSON manifest of written paths.

Pure stdlib — no third-party deps. Mirrors run_job.py conventions.
"""

from __future__ import annotations

import argparse
import json
import os
import re
import sys
from datetime import datetime, timezone
from pathlib import Path
from typing import Any

OKF_VERSION = "0.1"
SLIDE_EXTS = (".pptx", ".ppt")


class OkfError(RuntimeError):
    """Expected client-side failure."""


def eprint(message: str) -> None:
    print(message, file=sys.stderr)


def summarizer_home() -> Path:
    home = os.environ.get("SUMMARIZER_HOME") or os.environ.get("HOME")
    if not home:
        raise OkfError("Could not determine home directory (set HOME or SUMMARIZER_HOME).")
    return Path(home).expanduser() / ".summarizer"


def now_iso() -> str:
    return datetime.now(timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ")


# ----------------------------- loading -----------------------------


def load_output(args: argparse.Namespace) -> tuple[dict[str, Any], Path]:
    if args.input:
        path = Path(args.input).expanduser()
    elif args.job_id:
        path = summarizer_home() / "jobs" / args.job_id / "output.json"
    else:  # --latest
        history_path = summarizer_home() / "history.json"
        try:
            history = json.loads(history_path.read_text(encoding="utf-8"))
        except (OSError, json.JSONDecodeError) as exc:
            raise OkfError(f"Could not read history.json: {exc}") from exc
        completed = [
            job
            for job in history
            if isinstance(job, dict) and job.get("status") == "completed" and job.get("job_id")
        ]
        if not completed:
            raise OkfError("No completed jobs found in history.json.")
        path = summarizer_home() / "jobs" / completed[0]["job_id"] / "output.json"

    if not path.is_file():
        raise OkfError(f"output.json not found: {path}")
    try:
        data = json.loads(path.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError) as exc:
        raise OkfError(f"Could not parse {path}: {exc}") from exc
    if not isinstance(data, dict) or "document" not in data:
        raise OkfError(f"{path} is not a summarizer output (missing 'document').")
    return data, path


# ----------------------------- helpers -----------------------------


def slugify(value: str) -> str:
    value = re.sub(r"\.[A-Za-z0-9]{1,5}$", "", value)  # strip extension
    value = re.sub(r"[^A-Za-z0-9]+", "-", value).strip("-").lower()
    return value or "document"


def yaml_str(value: Any) -> str:
    """Emit a value as an always-double-quoted YAML scalar (newlines flattened).
    Always-quoting avoids every plain-scalar ambiguity (colons, leading
    dashes, numbers-as-strings, etc.) in machine-generated frontmatter."""
    text = str(value).replace("\r\n", "\n").replace("\n", " ").strip()
    text = text.replace("\\", "\\\\").replace('"', '\\"')
    return f'"{text}"'


def emit_frontmatter(pairs: list[tuple[str, Any]]) -> str:
    lines = ["---"]
    for key, value in pairs:
        if value is None or value == "" or value == []:
            continue
        if isinstance(value, bool):
            lines.append(f"{key}: {str(value).lower()}")
        elif isinstance(value, (int, float)):
            lines.append(f"{key}: {value}")
        elif isinstance(value, list):
            items = [str(item).strip() for item in value if str(item).strip()]
            if not items:
                continue
            lines.append(f"{key}: [{', '.join(yaml_str(item) for item in items)}]")
        else:
            lines.append(f"{key}: {yaml_str(value)}")
    lines.append("---")
    return "\n".join(lines)


def cell(text: Any) -> str:
    return str(text).replace("\\", "\\\\").replace("|", "\\|").replace("\n", " ").strip()


def table_to_md(table: list[list[Any]]) -> list[str]:
    rows = [row for row in table if isinstance(row, list)]
    ncol = max((len(row) for row in rows), default=0)
    if ncol == 0:
        return []
    lines: list[str] = []
    for i, row in enumerate(rows):
        cells = [cell(c) for c in row] + [""] * (ncol - len(row))
        lines.append("| " + " | ".join(cells) + " |")
        if i == 0:
            lines.append("|" + "|".join(["---"] * ncol) + "|")
    return lines


def page_number(page: dict[str, Any], idx: int) -> int:
    value = page.get("page_number")
    return value if isinstance(value, int) and value > 0 else idx + 1


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
        return (flat[:117] + "…") if len(flat) > 120 else flat
    return visual_text(page)[:120].strip()


def page_sections(page: dict[str, Any], level: int) -> list[str]:
    """Content sections for a page, headings at the given level. Shared by the
    bundle per-page files (level 1) and the single-file export (level 3)."""
    h = "#" * level
    topics = page.get("summary_topics") or []
    notes = page.get("summary_notes") or []
    img = visual_text(page)
    text = (page.get("text") or "").strip()
    tables = [t for t in (page.get("tables") or []) if t]
    lines: list[str] = []
    if topics:
        lines += ["", f"{h} Topics", *[f"- {t}" for t in topics]]
    if notes:
        lines += ["", f"{h} Notes", *[f"- {n}" for n in notes]]
    if img:
        lines += ["", f"{h} Visual Extraction", img]
    if text:
        lines += ["", f"{h} Extracted Text", text]
    if tables:
        rendered = [table_to_md(t) for t in tables]
        rendered = [r for r in rendered if r]
        if rendered:
            lines += ["", f"{h} Tables"]
            for i, md in enumerate(rendered):
                if len(rendered) > 1:
                    lines.append(f"{h}# Table {i + 1}")
                lines += md
    if not lines:
        lines += ["", "_(No extracted content for this page.)_"]
    return lines


def document_tags(pages: list[dict[str, Any]], cap: int = 30) -> list[str]:
    tags: list[str] = []
    seen: set[str] = set()
    for page in pages:
        for topic in page.get("summary_topics") or []:
            key = str(topic).strip()
            if key and key not in seen:
                seen.add(key)
                tags.append(key)
    return tags[:cap]


# ----------------------------- bundle emit -----------------------------


def render_page_file(
    page: dict[str, Any], idx: int, doc: dict[str, Any], unit: str, page_type: str
) -> tuple[str, str, int, str]:
    n = page_number(page, idx)
    filename = f"page-{n:04d}.md"
    description = first_description(page)
    relevancy = page.get("summary_relevancy")
    frontmatter = emit_frontmatter(
        [
            ("type", page_type),
            ("title", f"{unit} {n} — {doc.get('filename', 'document')}"),
            ("description", description),
            ("tags", page.get("summary_topics") or []),
            ("timestamp", now_iso()),
            ("summarizer_chunk_id", page.get("chunk_id")),
            ("summarizer_relevancy", relevancy if isinstance(relevancy, (int, float)) else None),
            ("summarizer_quality_validated", page.get("summary_quality_validated")),
            ("source_document_id", doc.get("document_id")),
            ("source_page_number", n),
        ]
    )
    body = [
        frontmatter,
        "",
        f"{unit} {n} of [{doc.get('filename', 'document')}](/document.md).",
    ]
    body += page_sections(page, level=1)
    return filename, "\n".join(body) + "\n", n, description


def to_okf_bundle(output: dict[str, Any], out_root: Path, args: argparse.Namespace) -> dict[str, Any]:
    doc = output["document"]
    pages = output.get("pages") or []
    filename = doc.get("filename", "document")
    is_slide = filename.lower().endswith(SLIDE_EXTS)
    unit = "Slide" if is_slide else "Page"
    page_type = args.page_type or ("Slide" if is_slide else "Document Page")
    doc_type = args.doc_type or "Document"

    bundle = out_root / slugify(filename)
    if bundle.exists() and any(bundle.iterdir()):
        suffix = str(doc.get("document_id", ""))[:8]
        if suffix:
            bundle = out_root / f"{slugify(filename)}-{suffix}"
    (bundle / "pages").mkdir(parents=True, exist_ok=True)

    written: list[str] = []

    def write(rel: str, content: str) -> None:
        (bundle / rel).write_text(content, encoding="utf-8")
        written.append(rel)

    entries: list[tuple[int, str, str]] = []
    for idx, page in enumerate(pages):
        fname, content, n, desc = render_page_file(page, idx, doc, unit, page_type)
        write(f"pages/{fname}", content)
        entries.append((n, fname, desc))

    pages_index = ["# Pages", ""]
    for n, fname, desc in entries:
        pages_index.append(f"* [{unit} {n}]({fname}) - {desc or 'No description.'}")
    write("pages/index.md", "\n".join(pages_index) + "\n")

    n_summarized = sum(
        1 for p in pages if (p.get("summary_notes") or p.get("summary_topics"))
    )
    metadata = doc.get("metadata") or {}
    doc_frontmatter = emit_frontmatter(
        [
            ("type", doc_type),
            ("title", metadata.get("title") or filename),
            (
                "description",
                f"{filename} — {doc.get('total_pages', len(pages))} pages, {n_summarized} summarized.",
            ),
            ("tags", document_tags(pages)),
            ("timestamp", now_iso()),
            ("source_document_id", doc.get("document_id")),
            ("source_filename", filename),
            ("source_total_pages", doc.get("total_pages")),
        ]
    )
    doc_body = [
        doc_frontmatter,
        "",
        f"Structured extraction of **{filename}** ({doc.get('total_pages', len(pages))} pages).",
        "",
        "# Pages",
    ]
    for n, fname, desc in entries:
        doc_body.append(f"* [{unit} {n}](/pages/{fname}) - {desc or 'No description.'}")
    write("document.md", "\n".join(doc_body) + "\n")

    root_index = [
        emit_frontmatter([("okf_version", OKF_VERSION)]),
        "",
        "# Bundle",
        "",
        f"* [{filename}](document.md) - Whole-document concept.",
        f"* [Pages](pages/) - {len(pages)} per-{unit.lower()} concept(s).",
    ]
    write("index.md", "\n".join(root_index) + "\n")

    if not args.no_log:
        log = [
            "# Directory Update Log",
            "",
            f"## {datetime.now(timezone.utc):%Y-%m-%d}",
            "",
            f"* **Creation**: Generated OKF bundle from {filename} "
            f"({doc.get('total_pages', len(pages))} pages).",
        ]
        write("log.md", "\n".join(log) + "\n")

    return {
        "format": "okf-bundle",
        "okf_version": OKF_VERSION,
        "bundle_dir": str(bundle),
        "files_written": written,
        "page_count": len(pages),
        "document_filename": filename,
    }


# ----------------------------- single-file emit -----------------------------


def to_okf_single(output: dict[str, Any], out_path: Path, args: argparse.Namespace) -> dict[str, Any]:
    doc = output["document"]
    pages = output.get("pages") or []
    filename = doc.get("filename", "document")
    is_slide = filename.lower().endswith(SLIDE_EXTS)
    unit = "Slide" if is_slide else "Page"
    metadata = doc.get("metadata") or {}
    n_summarized = sum(1 for p in pages if (p.get("summary_notes") or p.get("summary_topics")))

    frontmatter = emit_frontmatter(
        [
            ("type", args.doc_type or "Document"),
            ("title", metadata.get("title") or filename),
            (
                "description",
                f"{filename} — {doc.get('total_pages', len(pages))} pages, {n_summarized} summarized.",
            ),
            ("tags", document_tags(pages)),
            ("timestamp", now_iso()),
            ("source_document_id", doc.get("document_id")),
            ("source_filename", filename),
            ("source_total_pages", doc.get("total_pages")),
        ]
    )
    lines = [
        frontmatter,
        "",
        f"# {metadata.get('title') or filename}",
        "",
        f"Structured extraction of **{filename}** ({doc.get('total_pages', len(pages))} pages).",
    ]
    for idx, page in enumerate(pages):
        n = page_number(page, idx)
        lines += ["", f"## {unit} {n}"]
        lines += page_sections(page, level=3)

    if out_path.is_dir():
        out_path = out_path / f"{slugify(filename)}.okf.md"
    out_path.parent.mkdir(parents=True, exist_ok=True)
    out_path.write_text("\n".join(lines) + "\n", encoding="utf-8")
    return {
        "format": "okf-single",
        "file_written": str(out_path),
        "page_count": len(pages),
        "document_filename": filename,
    }


# ----------------------------- cli -----------------------------


def build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(
        description="Convert a Document Summarizer output.json into Open Knowledge Format (OKF v0.1)."
    )
    source = parser.add_mutually_exclusive_group(required=True)
    source.add_argument("--input", help="Path to an output.json")
    source.add_argument("--job-id", help="Resolve ~/.summarizer/jobs/<id>/output.json")
    source.add_argument(
        "--latest", action="store_true", help="Use the newest completed job in history.json"
    )
    parser.add_argument(
        "--out",
        help="Output location. Bundle: parent dir (default: <output.json dir>/okf). "
        "Single: file or dir (default: <output.json dir>/<slug>.okf.md).",
    )
    parser.add_argument(
        "--granularity",
        choices=["pages", "single"],
        default="pages",
        help="pages = OKF directory bundle (default); single = one markdown file",
    )
    parser.add_argument("--page-type", help="Override per-page OKF type (default: Slide/Document Page)")
    parser.add_argument("--doc-type", help="Override document OKF type (default: Document)")
    parser.add_argument("--no-log", action="store_true", help="Skip log.md in the bundle")
    return parser


def main() -> int:
    args = build_parser().parse_args()
    output, source_path = load_output(args)

    if args.granularity == "single":
        out_path = (
            Path(args.out).expanduser()
            if args.out
            else source_path.parent / f"{slugify(output['document'].get('filename', 'document'))}.okf.md"
        )
        manifest = to_okf_single(output, out_path, args)
    else:
        out_root = Path(args.out).expanduser() if args.out else source_path.parent / "okf"
        manifest = to_okf_bundle(output, out_root, args)

    manifest["source_output_json"] = str(source_path)
    print(json.dumps(manifest, indent=2))
    return 0


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except OkfError as exc:
        eprint(f"ERROR: {exc}")
        raise SystemExit(1)
