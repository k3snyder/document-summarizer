#!/usr/bin/env python3
"""Query a Document Summarizer output.json without dumping the whole file."""

from __future__ import annotations

import argparse
import base64
import binascii
import csv
import json
import re
import sys
from pathlib import Path
from typing import Any

from _common import (
    SkillError,
    first_description,
    json_dump,
    load_output,
    page_number,
    parse_page_range,
    regex_snippets,
    strip_images,
    visual_text,
)


FIELDS = {
    "chunk_id",
    "doc_title",
    "page_number",
    "text",
    "tables",
    "extraction_warnings",
    "html",
    "embedded_images",
    "image_base64",
    "image_text",
    "visual_text",
    "image_classifier",
    "summary_notes",
    "summary_topics",
    "summary_relevancy",
    "summary_quality_validated",
    "summary_notes_1",
    "summary_notes_2",
    "summary_notes_3",
    "summary_budget_exhausted",
}


def select_pages(output: dict[str, Any], spec: str | None) -> list[tuple[int, dict[str, Any]]]:
    pages = [page for page in output.get("pages") or [] if isinstance(page, dict)]
    selected_numbers = parse_page_range(spec) if spec else None
    selected: list[tuple[int, dict[str, Any]]] = []
    for idx, page in enumerate(pages):
        number = page_number(page, idx)
        if selected_numbers is None or number in selected_numbers:
            selected.append((number, page))
    return selected


def summary(output: dict[str, Any], pages: list[tuple[int, dict[str, Any]]]) -> dict[str, Any]:
    doc = output.get("document") or {}
    return {
        "document": {
            "document_id": doc.get("document_id"),
            "filename": doc.get("filename"),
            "total_pages": doc.get("total_pages"),
            "metadata": doc.get("metadata") or {},
        },
        "metrics": output.get("metrics"),
        "pages": [
            {"page_number": number, "description": first_description(page)}
            for number, page in pages
        ],
    }


def field_projection(pages: list[tuple[int, dict[str, Any]]], fields: str) -> list[dict[str, Any]]:
    requested = [field.strip() for field in fields.split(",") if field.strip()]
    unknown = sorted(set(requested) - FIELDS)
    if unknown:
        raise SkillError(f"Unknown field(s): {', '.join(unknown)}. Valid fields: {', '.join(sorted(FIELDS))}")
    rows: list[dict[str, Any]] = []
    for number, page in pages:
        row: dict[str, Any] = {"page_number": number}
        for field in requested:
            if field == "page_number":
                continue
            if field == "visual_text":
                row[field] = visual_text(page)
            else:
                row[field] = page.get(field)
        rows.append(row)
    return rows


def grep_pages(pages: list[tuple[int, dict[str, Any]]], pattern: str) -> list[dict[str, Any]]:
    compiled = re.compile(pattern, re.IGNORECASE)
    results: list[dict[str, Any]] = []
    for number, page in pages:
        haystack = "\n".join(
            part
            for part in [
                page.get("text") if isinstance(page.get("text"), str) else "",
                "\n".join(str(note) for note in (page.get("summary_notes") or [])),
                visual_text(page),
            ]
            if part
        )
        snippets = regex_snippets(compiled, haystack)
        if snippets:
            results.append({"page_number": number, "matches": snippets})
    return results


def topics(pages: list[tuple[int, dict[str, Any]]]) -> list[dict[str, Any]]:
    counts: dict[str, int] = {}
    for _, page in pages:
        for topic in page.get("summary_topics") or []:
            key = str(topic).strip()
            if key:
                counts[key] = counts.get(key, 0) + 1
    return [
        {"topic": topic, "pages": count}
        for topic, count in sorted(counts.items(), key=lambda item: (-item[1], item[0].lower()))
    ]


def stats(output: dict[str, Any], pages: list[tuple[int, dict[str, Any]]]) -> dict[str, Any]:
    return {
        "page_count": len(pages),
        "pages_with_vision": sum(1 for _, page in pages if visual_text(page)),
        "pages_with_summaries": sum(
            1 for _, page in pages if page.get("summary_notes") or page.get("summary_topics")
        ),
        "pages_with_tables": sum(1 for _, page in pages if page.get("tables")),
        "metrics": output.get("metrics"),
    }


def tables(pages: list[tuple[int, dict[str, Any]]], as_csv: bool, out_dir: str | None) -> Any:
    extracted: list[dict[str, Any]] = []
    for number, page in pages:
        for table_index, table in enumerate(page.get("tables") or []):
            extracted.append({"page_number": number, "table_index": table_index, "rows": table})
    if not as_csv:
        return extracted

    root = Path(out_dir or "tables").expanduser()
    root.mkdir(parents=True, exist_ok=True)
    written: list[str] = []
    for item in extracted:
        path = root / f"page-{item['page_number']:04d}-table-{item['table_index'] + 1}.csv"
        with path.open("w", encoding="utf-8", newline="") as handle:
            writer = csv.writer(handle)
            writer.writerows(item["rows"])
        written.append(str(path))
    return {"tables": len(extracted), "files_written": written}


def export_assets(
    output: dict[str, Any],
    pages: list[tuple[int, dict[str, Any]]],
    exports: list[str],
    out_dir: str | None,
) -> dict[str, Any]:
    root = Path(out_dir or "exports").expanduser()
    root.mkdir(parents=True, exist_ok=True)
    manifest: dict[str, Any] = {"out_dir": str(root), "exports": {}}
    doc = output.get("document") or {}

    if "tables" in exports:
        table_dir = root / "tables"
        table_dir.mkdir(parents=True, exist_ok=True)
        table_rows = []
        written = []
        for number, page in pages:
            for table_index, table in enumerate(page.get("tables") or []):
                item = {
                    "document_id": doc.get("document_id"),
                    "filename": doc.get("filename"),
                    "page_number": number,
                    "table_index": table_index,
                    "rows": table,
                }
                table_rows.append(item)
                csv_path = table_dir / f"page-{number:04d}-table-{table_index + 1}.csv"
                with csv_path.open("w", encoding="utf-8", newline="") as handle:
                    csv.writer(handle).writerows(table)
                written.append(str(csv_path))
        jsonl_path = table_dir / "tables.jsonl"
        with jsonl_path.open("w", encoding="utf-8") as handle:
            for item in table_rows:
                handle.write(json.dumps(item, ensure_ascii=True) + "\n")
        manifest["exports"]["tables"] = {
            "count": len(table_rows),
            "files_written": written + [str(jsonl_path)],
        }

    if "pages-jsonl" in exports:
        path = root / "pages.jsonl"
        with path.open("w", encoding="utf-8") as handle:
            for number, page in pages:
                handle.write(
                    json.dumps(
                        {
                            "page_number": number,
                            "text": page.get("text"),
                            "summary_notes": page.get("summary_notes"),
                            "summary_topics": page.get("summary_topics"),
                        },
                        ensure_ascii=True,
                    )
                    + "\n"
                )
        manifest["exports"]["pages-jsonl"] = {"files_written": [str(path)], "count": len(pages)}

    if "images" in exports:
        image_dir = root / "images"
        image_dir.mkdir(parents=True, exist_ok=True)
        written = []
        for number, page in pages:
            payload = page.get("image_base64")
            if not isinstance(payload, str) or not payload:
                continue
            try:
                data = base64.b64decode(payload, validate=True)
            except binascii.Error:
                continue
            ext = ".png" if data.startswith(b"\x89PNG") else ".jpg" if data.startswith(b"\xff\xd8") else ".bin"
            path = image_dir / f"page-{number:04d}{ext}"
            path.write_bytes(data)
            written.append(str(path))
        manifest["exports"]["images"] = {
            "count": len(written),
            "files_written": written,
            "hint": None if written else "Run with --keep-base64-images to export page images.",
        }

    return manifest


def render_markdown(data: Any) -> str:
    if isinstance(data, dict) and "pages" in data:
        lines = [
            f"# {data.get('document', {}).get('filename', 'document')}",
            "",
            f"Pages: {data.get('document', {}).get('total_pages', len(data.get('pages', [])))}",
            "",
        ]
        for page in data.get("pages", []):
            lines.append(f"- Page {page.get('page_number')}: {page.get('description', '')}")
        return "\n".join(lines) + "\n"
    return "```json\n" + json.dumps(data, indent=2, sort_keys=True) + "\n```\n"


def build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(description="Query a summarizer output.json compactly.")
    source = parser.add_mutually_exclusive_group(required=True)
    source.add_argument("--input", help="Path to an output.json")
    source.add_argument("--job-id", help="Resolve ~/.summarizer/jobs/<id>/output.json")
    source.add_argument("--latest", action="store_true", help="Use newest completed app History job")
    source.add_argument("--run", help="Resolve a CLI catalog run id, or 'latest'")
    parser.add_argument("--summary", action="store_true", help="Show document metadata and page one-liners")
    parser.add_argument("--pages", help="Page selector such as 2,5-9")
    parser.add_argument("--fields", help="Comma-separated page fields to return")
    parser.add_argument("--grep", help="Case-insensitive regex over text, notes, and visual text")
    parser.add_argument("--topics", action="store_true", help="Show deduplicated summary topics")
    parser.add_argument("--stats", action="store_true", help="Show compact document stats")
    parser.add_argument("--tables", action="store_true", help="Return extracted tables")
    parser.add_argument("--export", action="append", choices=["tables", "pages-jsonl", "images"], help="Export assets to --out (repeatable)")
    parser.add_argument("--as", dest="as_format", choices=["json", "csv"], default="json")
    parser.add_argument("--out", help="Output directory for CSV table export")
    parser.add_argument("--include-images", action="store_true", help="Allow base64 image payloads in output")
    parser.add_argument("--md", action="store_true", help="Render markdown instead of JSON")
    return parser


def run(args: argparse.Namespace) -> Any:
    output, source_path = load_output(args)
    pages = select_pages(output, args.pages)
    if args.fields:
        result: Any = {"source_output_json": str(source_path), "pages": field_projection(pages, args.fields)}
    elif args.grep:
        result = {"source_output_json": str(source_path), "matches": grep_pages(pages, args.grep)}
    elif args.topics:
        result = {"source_output_json": str(source_path), "topics": topics(pages)}
    elif args.stats:
        result = {"source_output_json": str(source_path), "stats": stats(output, pages)}
    elif args.tables:
        result = {
            "source_output_json": str(source_path),
            "tables": tables(pages, args.as_format == "csv", args.out),
        }
    elif args.export:
        result = {
            "source_output_json": str(source_path),
            "export": export_assets(output, pages, args.export, args.out),
        }
    else:
        result = summary(output, pages)
        result["source_output_json"] = str(source_path)

    return result if args.include_images else strip_images(result)


def main() -> int:
    args = build_parser().parse_args()
    result = run(args)
    if args.md:
        print(render_markdown(result), end="")
    else:
        json_dump(result)
    return 0


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except SkillError as exc:
        print(f"ERROR: {exc}", file=sys.stderr)
        raise SystemExit(2)
