#!/usr/bin/env python3
"""to_dataset.py — corpus-level dataset compiler over summarizer output.json files (roadmap F13).

The stock `query_result.py --export pages-jsonl` drops document identity, vision
text, tables, and relevancy — it cannot feed a cross-document corpus. This
compiler emits the factory's merged seeds file with FULL field projection,
following the reference implementation in the Gemma walkthrough v2 Appendix A
(`build_dataset.py seeds`), plus the content-hash keying the factory's
incremental-rebuild discipline requires.

Usage:
  python3 to_dataset.py seeds --inputs '~/corpus/**/*_output.json' --out data/
  python3 to_dataset.py seeds --inputs '~/a/*_output.json' --inputs '~/b/*_output.json' \
      --out data/ --min-relevancy 70
  python3 to_dataset.py entigraph --seeds data/seeds.jsonl --out data/
  python3 to_dataset.py qa --seeds data/seeds.jsonl --out data/
  python3 to_dataset.py judge --sft data/sft_domain.jsonl --seeds data/seeds.jsonl \
      --out data/
  python3 to_dataset.py probe --sft data/sft_domain.jsonl --out data/
  python3 to_dataset.py mix --sft data/sft_known.jsonl --cpt data/cpt_synth.jsonl \
      --general data/sft_general.jsonl --out data/

Outputs:
  <out>/seeds.jsonl          one record per kept page (full projection)
  <out>/seeds_manifest.json  per-document accounting, drop reasons, topic
                             inventory, and the amplification budget

Stdlib only.
"""
from __future__ import annotations

import argparse
import glob
import hashlib
import itertools
import json
import queue
import random
import re
import shutil
import subprocess
import sys
import tempfile
import threading
import time
import urllib.error
import urllib.request
from collections import Counter
from collections import defaultdict
from pathlib import Path

DEFAULT_MIN_RELEVANCY = 70
DEFAULT_CATALOG = Path.home() / ".summarizer" / "cli-runs.jsonl"
DEFAULT_GENERATOR_URLS = [
    "http://192.168.10.3:11440",
]
CODEX_MODEL = "gpt-5.5"
DEFAULT_QA_SYSTEM = (
    "You are the internal assistant. Answer from company knowledge accurately "
    "and directly."
)
# The walkthrough's planning constant: real tokens ~= pages x 500.
TOKENS_PER_PAGE_ESTIMATE = 500
# The F13 amplification planning base from the factory walkthrough.
AMPLIFY_REAL_TOKEN_BASE = 142_500

ENTI_PROMPT = """You are writing training text that internalizes an organization's documents.
Write ONE substantial passage (200-400 words) that connects these entities using ONLY the
source material below. Style for this passage: {style}.
ENTITIES: {entities}
SOURCE MATERIAL (pages mentioning them):
{material}
The passage must be factually grounded in the source material — no outside knowledge,
no invented specifics. Explore how the entities relate: causes, contrasts, implications,
processes. Output only the passage text."""

REPHRASE_PROMPT = """Rewrite the following page content as a {style}. Preserve every fact,
number, and name exactly; change only structure and phrasing. Output only the rewrite.
CONTENT:
{content}"""

# Structure lane (bug 2026-07-06-gate1-layout-facts-untaught-by-entigraph, fix F1):
# ENTI_PROMPT optimizes entity-association density and loses document geometry;
# same-doc pairing even blurs section membership. This prompt makes geometry the
# REQUIRED output, from ONE page only — never cross-page material.
STRUCT_PROMPT = """You are writing training text that internalizes the STRUCTURE of one
page of an organization's document. Write ONE passage (120-300 words) in the style of
a {style} that explicitly states the page's layout facts, using ONLY the page below:
- which document ("{filename}") and page number ({page_number}) this is
- what listings, tables, or sections the page contains and HOW MANY entries each has
- which section or grouping each named item belongs to (state memberships explicitly,
  e.g. "X appears under Y, not under Z" when the page shows distinct groups)
- the order in which entries appear, where the page shows an order
Every structural claim must come from the page content; no outside knowledge, no
invented items. Prefer exhaustive, explicit membership statements over prose flow.
PAGE CONTENT:
{content}
Output only the passage text."""

VISION_PROMPT = """You are writing training text that internalizes facts conveyed by
the VISION/OCR extraction of organization documents. Use ONLY the supplied page
records. Do not use eval questions, do not write quiz questions, and do not invent
facts beyond the record.

For each page record, write exactly {per_page} standalone prose passages. The passages
must teach the page's diagram/table/visual-text content as operational facts:
- component names, labels, node/edge mappings, flows, timelines, data-handling or
  security properties, numeric values, and grouped memberships shown in the vision text
- connect labels to meanings and processes; state what the diagram or visual content
  conveys, not merely that a diagram exists
- anchor each passage to the named document and page number
- avoid saying "the image shows" as the main fact; convert visual evidence into
  factual prose
- vary wording and angle across passages while preserving exact names and numbers

Return ONLY a JSON array with one object per input page:
[
  {{"index": 0, "passages": ["...", "..."]}},
  {{"index": 1, "passages": ["...", "..."]}}
]

PAGE RECORDS:
{records}"""

STYLES = [
    "internal memo",
    "analyst brief",
    "FAQ answer",
    "onboarding explainer",
    "meeting-notes recap",
    "Q&A dialogue between two colleagues",
    "training quiz with answers",
    "executive summary",
    "troubleshooting guide entry",
    "customer-facing email",
]

GEN_PROMPT = """You are creating fine-tuning data that teaches a model the content of an
internal document. Below is one page: its text, tables, and key facts.
PAGE ({filename}, page {page_number}):
{text}
TABLES:
{tables_md}
VISUAL TEXT:
{visual_text}
KEY FACTS:
{notes}
TOPICS: {topics}
For EACH key fact, generate {per_fact} question-answer pairs with genuinely different
phrasings and question types (direct lookup, table lookup, how/why application, edge case,
skeptical-executive challenge, aggregation across the page). Every answer must be fully
supported by the page content; no outside knowledge. For about 1 in 3 answers, attribute
the source ("According to {filename}..."). Answers: 2-5 sentences, direct.
Output ONLY a JSON array: [{{"question": "...", "answer": "..."}}]"""

JUDGE_PROMPT = """You are judging fine-tuning question-answer pairs against ONE source page.
Use ONLY the source page below. Do not use outside knowledge.

SOURCE PAGE:
{source}

QUESTION-ANSWER PAIRS:
{pairs}

Score EACH pair from 1-10 on:
- groundedness: every claim in the answer is supported by THIS page.
- clarity: the question is clear and the answer directly addresses it.
- usefulness: an employee would plausibly ask this about the page. Questions about
  the page's visual layout, design, colors, icons, or slide aesthetics get usefulness <= 3.

Return ONLY a JSON array:
[{{"i": <number>, "groundedness": n, "clarity": n, "usefulness": n}}]"""


def eprint(*args) -> None:
    print(*args, file=sys.stderr)


FILE_WRITE_LOCK = threading.Lock()


class GenerationError(RuntimeError):
    """Raised after one generator request exhausts its retry budget."""


class HttpChatClient:
    def __init__(
        self,
        base_url: str,
        temperature: float = 1.0,
        top_p: float = 0.95,
        top_k: int = 64,
        timeout: int = 300,
    ) -> None:
        self.base_url = base_url.rstrip("/")
        self.temperature = temperature
        self.top_p = top_p
        self.top_k = top_k
        self.timeout = timeout
        self.label = self.base_url
        self.provenance = self.base_url

    def generate(self, prompt: str, max_tokens: int) -> tuple[str, dict]:
        body = {
            "model": "model.gguf",
            "messages": [{"role": "user", "content": prompt}],
            "temperature": self.temperature,
            "top_p": self.top_p,
            "top_k": self.top_k,
            "max_tokens": max_tokens,
        }
        data = json.dumps(body).encode("utf-8")
        last_error: Exception | None = None
        for attempt, delay in enumerate((0, 5, 20), start=1):
            if delay:
                time.sleep(delay)
            request = urllib.request.Request(
                f"{self.base_url}/v1/chat/completions",
                data=data,
                headers={"Content-Type": "application/json"},
                method="POST",
            )
            try:
                with urllib.request.urlopen(request, timeout=self.timeout) as response:
                    payload = json.loads(response.read().decode("utf-8"))
                content = payload["choices"][0]["message"]["content"]
                return str(content), payload.get("usage") or {}
            except (
                OSError,
                TimeoutError,
                KeyError,
                IndexError,
                json.JSONDecodeError,
                urllib.error.URLError,
            ) as err:
                last_error = err
                if attempt == 3:
                    break
        raise GenerationError(str(last_error) if last_error else "generation failed")


class CodexChatClient:
    def __init__(
        self,
        codex_cmd: str,
        effort: str,
        worker_index: int,
        timeout: int = 240,
    ) -> None:
        self.codex_cmd = codex_cmd
        self.effort = effort
        self.worker_index = worker_index
        self.timeout = timeout
        self.label = f"codex:{worker_index}"
        self.provenance = f"codex:{CODEX_MODEL}:{effort}"
        self.temp_dir = Path(tempfile.mkdtemp(prefix=f"to_dataset_codex_{worker_index}_"))

    def generate(self, prompt: str, max_tokens: int) -> tuple[str, dict]:
        del max_tokens
        last_error: Exception | None = None
        for attempt, delay in enumerate((0, 5, 20), start=1):
            if delay:
                time.sleep(delay)
            tmp = tempfile.NamedTemporaryFile(
                prefix="last_message_",
                suffix=".txt",
                dir=self.temp_dir,
                delete=False,
            )
            output_path = Path(tmp.name)
            tmp.close()
            try:
                command = [
                    self.codex_cmd,
                    "exec",
                    "--skip-git-repo-check",
                    "--ephemeral",
                    "-s",
                    "read-only",
                    "-m",
                    CODEX_MODEL,
                    "-c",
                    f"model_reasoning_effort={self.effort}",
                    "--output-last-message",
                    str(output_path),
                    "--",
                    prompt,
                ]
                result = subprocess.run(
                    command,
                    capture_output=True,
                    text=True,
                    timeout=self.timeout,
                    cwd=self.temp_dir,
                    check=False,
                )
                if result.returncode != 0:
                    detail = (result.stderr or result.stdout or "").strip()
                    raise RuntimeError(
                        f"codex exec exited {result.returncode}: {detail[-500:]}"
                    )
                text = output_path.read_text().strip()
                if not text:
                    raise RuntimeError("codex exec produced no final message")
                return text, {
                    "completion_tokens": round(word_count(text) * 1.35),
                    "usage_estimated": True,
                }
            except (
                OSError,
                RuntimeError,
                subprocess.TimeoutExpired,
            ) as err:
                last_error = err
                if attempt == 3:
                    break
            finally:
                try:
                    output_path.unlink()
                except OSError:
                    pass
        raise GenerationError(str(last_error) if last_error else "codex generation failed")

    def close(self) -> None:
        shutil.rmtree(self.temp_dir, ignore_errors=True)


def generator_urls(values: list[str] | None) -> list[str]:
    using_default = values is None
    if values is None:
        values = DEFAULT_GENERATOR_URLS
    urls = [
        v.rstrip("/")
        for v in values
        if v.strip() and v.strip().upper() != "SKIP"
    ]
    if urls or not using_default:
        return urls
    return list(DEFAULT_GENERATOR_URLS)


def build_generator_clients(args: argparse.Namespace, urls: list[str]) -> list:
    codex_workers = getattr(args, "codex_workers", 0)
    if codex_workers < 0:
        eprint("--codex-workers must be >= 0")
        return []
    clients = [
        HttpChatClient(
            url,
            temperature=args.temperature,
            top_p=args.top_p,
            top_k=args.top_k,
        )
        for url in urls
    ]
    for index in range(codex_workers):
        clients.append(
            CodexChatClient(
                codex_cmd=args.codex_cmd,
                effort=args.codex_effort,
                worker_index=index,
            )
        )
    return clients


def load_jsonl(path: Path) -> list[dict]:
    if not path.exists():
        return []
    records = []
    for line in path.read_text().splitlines():
        if not line.strip():
            continue
        try:
            records.append(json.loads(line))
        except json.JSONDecodeError:
            continue
    return records


def append_jsonl(path: Path, record: dict) -> None:
    with FILE_WRITE_LOCK:
        with path.open("a") as f:
            f.write(json.dumps(record) + "\n")
            f.flush()


def scan_done_keys(paths: list[Path]) -> set[str]:
    done = set()
    for path in paths:
        for record in load_jsonl(path):
            key = record.get("key")
            if key:
                done.add(str(key))
            batch_key = record.get("batch_key")
            if batch_key:
                done.add(str(batch_key))
    return done


def failed_count(path: Path) -> int:
    if not path.exists():
        return 0
    return sum(1 for line in path.read_text().splitlines() if line.strip())


def output_counts(out_dir: Path) -> tuple[Counter, Counter]:
    counts: Counter = Counter()
    tokens: Counter = Counter()
    for path, default_kind in (
        (out_dir / "cpt_synth.jsonl", None),
        (out_dir / "sft_domain.jsonl", "qa"),
        (out_dir / "sft_judged.jsonl", "judged_accepted"),
        (out_dir / "sft_judge_rejected.jsonl", "judged_rejected"),
        (out_dir / "sft_known.jsonl", "probe_known"),
        (out_dir / "sft_unknown.jsonl", "probe_unknown"),
    ):
        for record in load_jsonl(path):
            counts[record.get("kind") or default_kind or "unknown"] += 1
            gen_tokens = record.get("gen_tokens")
            if isinstance(gen_tokens, int):
                tokens["completion_tokens"] += gen_tokens
    return counts, tokens


def usage_counter(usage: dict) -> Counter:
    tokens: Counter = Counter()
    for key in ("prompt_tokens", "completion_tokens", "total_tokens"):
        value = usage.get(key)
        if isinstance(value, int):
            tokens[key] += value
    return tokens


def merge_usage(usages: list[dict]) -> dict:
    merged: Counter = Counter()
    estimated = False
    for usage in usages:
        merged.update(usage_counter(usage))
        estimated = estimated or bool(usage.get("usage_estimated"))
    result = dict(merged)
    if estimated:
        result["usage_estimated"] = True
    return result


class RunState:
    def __init__(
        self,
        out_dir: Path,
        endpoints: list[str],
        total: int,
        skipped: int,
    ) -> None:
        self.out_dir = out_dir
        self.endpoints = endpoints
        self.total = total
        self.skipped = skipped
        self.completed = skipped
        self.failed = failed_count(out_dir / "failed_items.jsonl")
        self.rejected = 0
        self.start = time.time()
        self.lock = threading.Lock()
        counts, tokens = output_counts(out_dir)
        manifest = out_dir / "amplify_manifest.json"
        if manifest.exists():
            try:
                previous = json.loads(manifest.read_text())
                previous_tokens = {
                    k: v
                    for k, v in (previous.get("token_totals") or {}).items()
                    if isinstance(v, int)
                }
                if previous_tokens:
                    tokens = Counter(previous_tokens)
            except json.JSONDecodeError:
                pass
        self.counts = counts
        self.tokens = tokens
        self.estimated_tokens = False
        if manifest.exists():
            try:
                previous = json.loads(manifest.read_text())
                self.estimated_tokens = bool(previous.get("estimated_tokens"))
            except json.JSONDecodeError:
                pass
        self.new_completion_tokens = 0

    def record_success(
        self,
        kind: str,
        usage: dict,
        records: int = 1,
        rejected: int = 0,
    ) -> None:
        with self.lock:
            self.counts[kind] += records
            self.rejected += rejected
            self.estimated_tokens = self.estimated_tokens or bool(
                usage.get("usage_estimated")
            )
            delta = usage_counter(usage)
            self.tokens.update(delta)
            self.new_completion_tokens += delta.get("completion_tokens", 0)
            self.completed += 1
            self._maybe_report_locked()

    def record_result_counts(
        self,
        result_counts: dict[str, int],
        usage: dict,
        rejected: int = 0,
    ) -> None:
        with self.lock:
            for kind, count in result_counts.items():
                self.counts[kind] += count
            self.rejected += rejected
            self.estimated_tokens = self.estimated_tokens or bool(
                usage.get("usage_estimated")
            )
            delta = usage_counter(usage)
            self.tokens.update(delta)
            self.new_completion_tokens += delta.get("completion_tokens", 0)
            self.completed += 1
            self._maybe_report_locked()

    def record_failure(self, rejected: bool = False) -> None:
        with self.lock:
            self.failed += 1
            self.rejected += int(rejected)
            self.completed += 1
            self._maybe_report_locked()

    def _maybe_report_locked(self) -> None:
        if self.completed % 25 == 0 or self.completed == self.total:
            self._progress_locked()
        if self.completed % 100 == 0 or self.completed == self.total:
            self._manifest_locked()

    def _progress_locked(self) -> None:
        elapsed = max(0.001, time.time() - self.start)
        remaining = max(0, self.total - self.completed)
        rate_items = max(0.001, (self.completed - self.skipped) / elapsed)
        eta = remaining / rate_items
        generated = self.tokens.get("completion_tokens", 0)
        tok_s = self.new_completion_tokens / elapsed
        eprint(
            f"progress: {self.completed}/{self.total}, failed {self.failed}, "
            f"tok generated {generated}, tok/s {tok_s:.1f}, ETA {eta:.0f}s"
        )

    def _manifest_locked(self) -> None:
        completion_tokens = self.tokens.get("completion_tokens", 0)
        manifest = {
            "counts_by_kind": dict(sorted(self.counts.items())),
            "token_totals": dict(sorted(self.tokens.items())),
            "budget_fraction": completion_tokens / AMPLIFY_REAL_TOKEN_BASE,
            "real_token_base": AMPLIFY_REAL_TOKEN_BASE,
            "estimated_tokens": self.estimated_tokens,
            "failed_count": self.failed,
            "rejected_count": self.rejected,
            "wall_clock_seconds": round(time.time() - self.start, 3),
            "endpoints": self.endpoints,
            "work": {
                "total": self.total,
                "completed": self.completed,
                "skipped_existing": self.skipped,
                "queued": self.total - self.skipped,
            },
        }
        if self.estimated_tokens:
            manifest["estimated_tokens_note"] = (
                "Some completion token counts were estimated from codex exec output "
                "word counts because that lane does not return usage."
            )
        (self.out_dir / "amplify_manifest.json").write_text(
            json.dumps(manifest, indent=2, sort_keys=True)
        )

    def write_manifest(self) -> None:
        with self.lock:
            self._manifest_locked()

    def write_progress(self) -> None:
        with self.lock:
            self._progress_locked()


def log_failed_item(out_dir: Path, item: dict, reason: str, endpoint: str | None) -> None:
    append_jsonl(
        out_dir / "failed_items.jsonl",
        {
            "key": item.get("key"),
            "kind": item.get("kind"),
            "reason": reason,
            "endpoint": endpoint,
            "ts": round(time.time(), 3),
        },
    )


def run_parallel_items(
    items: list[dict],
    done_keys: set[str],
    out_dir: Path,
    clients: list,
    args: argparse.Namespace,
    process_item,
) -> int:
    if not clients:
        eprint("no generator clients configured")
        return 2
    remaining = [item for item in items if item["key"] not in done_keys]
    labels = [client.label for client in clients]
    state = RunState(out_dir, labels, len(items), len(items) - len(remaining))
    if not remaining:
        state.write_progress()
        state.write_manifest()
        return 0

    work: queue.Queue = queue.Queue()
    for item in remaining:
        work.put(item)
    stop = threading.Event()

    def worker(client) -> None:
        try:
            while not stop.is_set():
                try:
                    item = work.get_nowait()
                except queue.Empty:
                    return
                try:
                    process_item(item, client, client.label, state)
                except GenerationError as err:
                    log_failed_item(out_dir, item, str(err), client.label)
                    state.record_failure()
                except Exception as err:
                    log_failed_item(
                        out_dir,
                        item,
                        f"{type(err).__name__}: {err}",
                        client.label,
                    )
                    state.record_failure()
                finally:
                    work.task_done()
        finally:
            close = getattr(client, "close", None)
            if close:
                close()

    threads = [
        threading.Thread(target=worker, args=(client,), daemon=True)
        for client in clients
    ]
    for thread in threads:
        thread.start()
    try:
        while any(thread.is_alive() for thread in threads):
            for thread in threads:
                thread.join(timeout=0.2)
    except KeyboardInterrupt:
        stop.set()
        eprint("interrupted; append-only outputs can be resumed with the same command")
        state.write_manifest()
        return 130
    state.write_manifest()
    return 0


def word_count(text: str) -> int:
    return len(re.findall(r"\S+", text or ""))


def normalize_entity(value: object) -> str:
    return str(value or "").strip().lower()


def page_identity(seed: dict) -> str:
    return f"{seed.get('document_id')}:{seed.get('page_number')}"


def source_material(seeds: list[dict], idxs: list[int]) -> str:
    chunks = []
    for idx in idxs[:4]:
        seed = seeds[idx]
        chunks.append(
            f"[{seed.get('filename')} p{seed.get('page_number')}]\n"
            f"{(seed.get('text') or '')[:1500]}\n"
            f"{seed.get('tables_md') or ''}\n"
            f"{seed.get('visual_text') or ''}"
        )
    return "\n---\n".join(chunks)


def page_content(seed: dict) -> str:
    return "\n\n".join(
        part
        for part in (
            seed.get("text") or "",
            seed.get("tables_md") or "",
            seed.get("visual_text") or "",
        )
        if part
    )


def clean_json_array(raw: str) -> list:
    match = re.search(r"\[.*\]", raw, re.DOTALL)
    if not match:
        raise ValueError("no JSON array found")
    parsed = json.loads(match.group(0))
    if not isinstance(parsed, list):
        raise ValueError("JSON payload is not an array")
    return parsed


def question_hash(question: str) -> str:
    normalized = re.sub(r"\W", "", question.lower())
    return hashlib.sha256(normalized.encode("utf-8")).hexdigest()


def token_overlap(gold_answer: str, candidate: str) -> float:
    gold = set(re.findall(r"[A-Za-z0-9%$.]{3,}", gold_answer.lower()))
    got = set(re.findall(r"[A-Za-z0-9%$.]{3,}", candidate.lower()))
    return len(gold & got) / max(1, len(gold))


def visual_text(page: dict) -> str:
    """Merge the primary vision pass with the optional multi-sample passes."""
    parts = [page.get("image_text") or ""]
    parts += [page.get(f"image_text_{i}") or "" for i in (1, 2, 3)]
    return "\n".join(p for p in parts if p).strip()


def tables_md(page: dict) -> str:
    """Render tables -> rows -> string cells as markdown. Markdown, never CSV:
    header markers are worth ~16 points of table comprehension in training."""
    out = []
    for table in page.get("tables") or []:
        if not table:
            continue
        header, *rows = table
        out.append("| " + " | ".join(header) + " |")
        out.append("|" + "---|" * len(header))
        out += ["| " + " | ".join(row) + " |" for row in rows]
        out.append("")
    return "\n".join(out).strip()


def notes_passes(page: dict) -> list[list[str]]:
    """The 3 raw detailed-extraction passes, when present — free
    rejection-sampling / preference material downstream."""
    passes = []
    for i in (1, 2, 3):
        value = page.get(f"summary_notes_{i}")
        if value:
            passes.append(value)
    return passes


def load_catalog(path: Path) -> dict[str, dict]:
    """Newest catalog record per output_json_path — the source-content-hash
    join (same discipline as the summarizer cache and PacasDB publisher)."""
    catalog: dict[str, dict] = {}
    if not path.exists():
        return catalog
    for line in path.read_text().splitlines():
        try:
            record = json.loads(line)
        except json.JSONDecodeError:
            continue
        output_path = record.get("output_json_path")
        if output_path:
            catalog[str(Path(output_path).expanduser())] = record
    return catalog


def page_drop_reason(page: dict, min_relevancy: int, keep_warned: bool) -> str | None:
    if page.get("extraction_warnings") and not keep_warned:
        return "extraction_warnings"
    relevancy = page.get("summary_relevancy")
    if relevancy is not None and relevancy < min_relevancy:
        return "low_relevancy"
    has_content = bool(
        (page.get("text") or "").strip()
        or visual_text(page)
        or (page.get("tables") or [])
    )
    if not has_content:
        return "empty"
    return None


def cmd_seeds(args: argparse.Namespace) -> int:
    out_dir = Path(args.out).expanduser()
    out_dir.mkdir(parents=True, exist_ok=True)
    seeds_path = out_dir / "seeds.jsonl"
    manifest_path = out_dir / "seeds_manifest.json"
    catalog = load_catalog(Path(args.catalog).expanduser())

    paths: list[str] = []
    for pattern in args.inputs:
        paths += glob.glob(str(Path(pattern).expanduser()), recursive=True)
    paths = sorted(set(paths))
    if not paths:
        eprint("no output.json files matched --inputs")
        return 2

    documents = []
    topics = Counter()
    total_kept = 0
    with seeds_path.open("w") as sf:
        for path in paths:
            resolved = str(Path(path).expanduser())
            try:
                doc = json.loads(Path(resolved).read_text())
            except (OSError, json.JSONDecodeError) as err:
                eprint(f"skip {path}: {err}")
                continue
            meta = doc.get("document") or {}
            catalog_row = catalog.get(resolved, {})
            source_sha = catalog_row.get("input_sha256") or hashlib.sha256(
                Path(resolved).read_bytes()
            ).hexdigest()
            drops = Counter()
            kept = 0
            pages_with_tables = 0
            pages_with_vision = 0
            for page in doc.get("pages") or []:
                reason = page_drop_reason(page, args.min_relevancy, args.keep_warned)
                if reason:
                    drops[reason] += 1
                    continue
                page_tables = tables_md(page)
                page_vision = visual_text(page)
                pages_with_tables += bool(page_tables)
                pages_with_vision += bool(page_vision)
                for topic in page.get("summary_topics") or []:
                    topics[topic.strip().lower()] += 1
                sf.write(
                    json.dumps(
                        {
                            "document_id": meta.get("document_id"),
                            "filename": meta.get("filename"),
                            "source_sha256": source_sha,
                            "config_hash": catalog_row.get("config_hash"),
                            "page_number": page.get("page_number"),
                            "text": page.get("text") or "",
                            "visual_text": page_vision,
                            "tables_md": page_tables,
                            "summary_notes": page.get("summary_notes") or [],
                            "summary_notes_passes": notes_passes(page),
                            "summary_topics": page.get("summary_topics") or [],
                            "summary_relevancy": page.get("summary_relevancy"),
                            "summary_quality_validated": page.get(
                                "summary_quality_validated"
                            ),
                        }
                    )
                    + "\n"
                )
                kept += 1
            total_kept += kept
            documents.append(
                {
                    "filename": meta.get("filename"),
                    "document_id": meta.get("document_id"),
                    "source_sha256": source_sha,
                    "output_json": resolved,
                    "pages_total": len(doc.get("pages") or []),
                    "pages_kept": kept,
                    "dropped": dict(drops),
                    "pages_with_tables": pages_with_tables,
                    "pages_with_vision": pages_with_vision,
                }
            )

    real_tokens = total_kept * TOKENS_PER_PAGE_ESTIMATE
    manifest = {
        "documents": documents,
        "totals": {
            "documents": len(documents),
            "pages_kept": total_kept,
            "pages_dropped": sum(
                sum(d["dropped"].values()) for d in documents
            ),
            "distinct_topics": len(topics),
            "top_topics": topics.most_common(25),
        },
        "amplification_budget": {
            "real_tokens_estimate": real_tokens,
            "synthetic_target_50x": real_tokens * 50,
            "synthetic_target_100x": real_tokens * 100,
            "note": "real tokens ~= kept pages x 500 (walkthrough Part 3.2); "
            "synthetic CPT target is 50-100x real, log-linear returns to ~350x",
        },
        "filters": {
            "min_relevancy": args.min_relevancy,
            "keep_warned": bool(args.keep_warned),
        },
    }
    manifest_path.write_text(json.dumps(manifest, indent=2))
    eprint(
        f"seeds: {total_kept} pages from {len(documents)} docs -> {seeds_path}\n"
        f"manifest: {manifest_path}\n"
        f"amplification budget: ~{real_tokens:,} real tokens -> "
        f"{real_tokens * 50:,}-{real_tokens * 100:,} synthetic target"
    )
    return 0


def load_seeds(path: Path, limit_pages: int | None = None) -> list[dict]:
    seeds = load_jsonl(path)
    if limit_pages is not None:
        return seeds[:limit_pages]
    return seeds


def build_judge_clients(args: argparse.Namespace) -> list:
    urls = generator_urls(args.generator_url) if args.generator_url is not None else []
    return build_generator_clients(args, urls)


def seed_content_index(seeds: list[dict]) -> dict[tuple[str, object], str]:
    index = {}
    for seed in seeds:
        index[(str(seed.get("document_id")), seed.get("page_number"))] = page_content(seed)
    return index


def sft_source_key(record: dict) -> tuple[str, object]:
    source = record.get("_source") or {}
    return str(source.get("document_id")), source.get("page_number")


def message_content(record: dict, role: str) -> str:
    for message in record.get("messages") or []:
        if message.get("role") == role:
            return str(message.get("content") or "")
    return ""


def judge_items(sft_records: list[dict], batch_size: int) -> list[dict]:
    grouped: dict[tuple[str, object], list[dict]] = {}
    order: list[tuple[str, object]] = []
    for record in sft_records:
        key = sft_source_key(record)
        if key not in grouped:
            grouped[key] = []
            order.append(key)
        grouped[key].append(record)

    items = []
    for source_key in order:
        records = grouped[source_key]
        for chunk_index, start in enumerate(range(0, len(records), batch_size)):
            doc_id, page_number = source_key
            items.append(
                {
                    "key": f"judge:{doc_id}:{page_number}:{chunk_index}",
                    "kind": "judge",
                    "document_id": doc_id,
                    "page_number": page_number,
                    "records": records[start : start + batch_size],
                }
            )
    return items


def judge_prompt(source: str, records: list[dict]) -> str:
    rendered = []
    for index, record in enumerate(records, start=1):
        rendered.append(
            f"{index}. QUESTION: {message_content(record, 'user')}\n"
            f"ANSWER: {message_content(record, 'assistant')}"
        )
    return JUDGE_PROMPT.format(
        source=(source or "(missing source page content)")[:6000],
        pairs="\n\n".join(rendered),
    )


def score_value(score: dict, field: str) -> int | None:
    value = score.get(field)
    if isinstance(value, bool):
        return None
    try:
        numeric = int(float(value))
    except (TypeError, ValueError):
        return None
    return max(1, min(10, numeric))


def index_judge_scores(scores: list) -> dict[int, dict]:
    indexed = {}
    for score in scores:
        if not isinstance(score, dict):
            continue
        try:
            item_index = int(score.get("i"))
        except (TypeError, ValueError):
            continue
        indexed[item_index] = score
    return indexed


def with_judge_key(record: dict, judge_key: str) -> dict:
    out = dict(record)
    previous_key = out.get("key")
    source = dict(out.get("_source") or {})
    if previous_key:
        source.setdefault("sft_key", previous_key)
    out["_source"] = source
    out["key"] = judge_key
    return out


def judge_record(
    record: dict,
    judge_key: str,
    score: dict | None,
    provenance: str,
    accept_min: int,
) -> tuple[dict, bool]:
    out = with_judge_key(record, judge_key)
    if score is None:
        out["judge"] = {
            "groundedness": None,
            "clarity": None,
            "usefulness": None,
            "judge_model": provenance,
        }
        out["reject_reason"] = "judge_no_score"
        return out, False

    groundedness = score_value(score, "groundedness")
    clarity = score_value(score, "clarity")
    usefulness = score_value(score, "usefulness")
    out["judge"] = {
        "groundedness": groundedness,
        "clarity": clarity,
        "usefulness": usefulness,
        "judge_model": provenance,
    }
    if groundedness is None or clarity is None or usefulness is None:
        out["reject_reason"] = "judge_no_score"
        return out, False
    accepted = (
        groundedness >= accept_min
        and usefulness >= accept_min
        and clarity >= accept_min - 1
    )
    if not accepted:
        reasons = []
        if groundedness < accept_min:
            reasons.append("low_groundedness")
        if usefulness < accept_min:
            reasons.append("low_usefulness")
        if clarity < accept_min - 1:
            reasons.append("low_clarity")
        out["reject_reason"] = ",".join(reasons)
    return out, accepted


def entity_indexes(seeds: list[dict]) -> tuple[dict, dict, dict, dict]:
    ent_pages: dict[str, list[int]] = defaultdict(list)
    ent_page_ids: dict[str, set[str]] = defaultdict(set)
    ent_docs: dict[str, set[str]] = defaultdict(set)
    doc_entities: dict[str, set[str]] = defaultdict(set)
    for idx, seed in enumerate(seeds):
        doc_id = str(seed.get("document_id"))
        page_id = page_identity(seed)
        for topic in seed.get("summary_topics") or []:
            entity = normalize_entity(topic)
            if not entity:
                continue
            ent_pages[entity].append(idx)
            ent_page_ids[entity].add(page_id)
            ent_docs[entity].add(doc_id)
            doc_entities[doc_id].add(entity)
    return ent_pages, ent_page_ids, ent_docs, doc_entities


def material_indexes(
    entities: list[str],
    ent_pages: dict[str, list[int]],
    ent_docs: dict[str, set[str]],
    seeds: list[dict],
) -> list[int]:
    page_sets = [set(ent_pages[e]) for e in entities]
    common_pages = set.intersection(*page_sets) if page_sets else set()
    union_pages = set.union(*page_sets) if page_sets else set()
    selected: list[int] = []
    if common_pages:
        selected = sorted(common_pages)
    else:
        doc_sets = [ent_docs[e] for e in entities]
        common_docs = set.intersection(*doc_sets) if doc_sets else set()
        if common_docs:
            selected = sorted(
                idx
                for idx in union_pages
                if str(seeds[idx].get("document_id")) in common_docs
            )
    selected += [idx for idx in sorted(union_pages) if idx not in set(selected)]
    return selected[:4]


def entigraph_items(seeds: list[dict], args: argparse.Namespace) -> list[dict]:
    ent_pages, ent_page_ids, ent_docs, doc_entities = entity_indexes(seeds)
    entities = sorted(ent_pages)
    rng = random.Random(13)
    co_pairs = []
    cross_pairs = []
    for a, b in itertools.combinations(entities, 2):
        same_page = bool(ent_page_ids[a] & ent_page_ids[b])
        same_doc = bool(ent_docs[a] & ent_docs[b])
        if same_page or same_doc:
            co_pairs.append((a, b))
        elif len(set(ent_pages[a])) >= 2 and len(set(ent_pages[b])) >= 2:
            cross_pairs.append((a, b))
    rng.shuffle(co_pairs)
    rng.shuffle(cross_pairs)
    pairs = co_pairs[: args.pairs]
    cross_used = []
    if len(pairs) < args.pairs:
        cross_used = cross_pairs[: args.pairs - len(pairs)]
        pairs += cross_used
    eprint(
        "entigraph pair split: "
        f"cooccurring={len(pairs) - len(cross_used)}, "
        f"cross_document={len(cross_used)}, requested={args.pairs}"
    )

    triple_set = set()
    for doc_ents in doc_entities.values():
        for triple in itertools.combinations(sorted(doc_ents), 3):
            triple_set.add(triple)
    triples = sorted(triple_set)
    rng.shuffle(triples)
    triples = triples[: args.triples]

    items: list[dict] = []
    for a, b in pairs:
        entities_for_item = [a, b]
        items.append(
            {
                "key": f"pair:{a}|{b}",
                "kind": "entigraph",
                "entities": entities_for_item,
                "style": STYLES[len(items) % len(STYLES)],
                "idxs": material_indexes(entities_for_item, ent_pages, ent_docs, seeds),
            }
        )
    for triple in triples:
        entities_for_item = list(triple)
        items.append(
            {
                "key": "triple:" + "|".join(entities_for_item),
                "kind": "entigraph3",
                "entities": entities_for_item,
                "style": STYLES[len(items) % len(STYLES)],
                "idxs": material_indexes(entities_for_item, ent_pages, ent_docs, seeds),
            }
        )
    for seed in seeds:
        styles = rng.sample(STYLES, min(args.rephrasings, len(STYLES)))
        for style in styles:
            items.append(
                {
                    "key": f"rephrase:{seed.get('document_id')}:"
                    f"{seed.get('page_number')}:{style}",
                    "kind": "rephrase",
                    "style": style,
                    "seed": seed,
                }
            )
    return items


def cmd_entigraph(args: argparse.Namespace) -> int:
    out_dir = Path(args.out).expanduser()
    out_dir.mkdir(parents=True, exist_ok=True)
    out = out_dir / "cpt_synth.jsonl"
    seeds = load_seeds(Path(args.seeds).expanduser())
    items = entigraph_items(seeds, args)
    done = scan_done_keys([out])
    clients = build_generator_clients(args, generator_urls(args.generator_url))

    def process(item: dict, client, endpoint: str, state: RunState) -> None:
        if item["kind"] in {"entigraph", "entigraph3"}:
            prompt = ENTI_PROMPT.format(
                style=item["style"],
                entities=", ".join(item["entities"]),
                material=source_material(seeds, item["idxs"]),
            )
            minimum_words = 80
        else:
            prompt = REPHRASE_PROMPT.format(
                style=item["style"],
                content=page_content(item["seed"])[:4000],
            )
            minimum_words = 50
        text, usage = client.generate(prompt, args.max_tokens)
        text = text.strip()
        if word_count(text) < minimum_words:
            log_failed_item(
                out_dir,
                item,
                f"rejected: output under {minimum_words} words",
                endpoint,
            )
            state.record_failure(rejected=True)
            return
        gen_tokens = usage.get("completion_tokens")
        if not isinstance(gen_tokens, int):
            gen_tokens = 0
        record = {
            "key": item["key"],
            "text": text,
            "kind": item["kind"],
            "style": item["style"],
            "gen_tokens": gen_tokens,
            "generator": client.provenance,
        }
        if item["kind"] == "rephrase":
            seed = item["seed"]
            record["source"] = {
                "document_id": seed.get("document_id"),
                "page_number": seed.get("page_number"),
            }
        else:
            record["entities"] = item["entities"]
        append_jsonl(out, record)
        state.record_success(item["kind"], usage)

    return run_parallel_items(items, done, out_dir, clients, args, process)


def structure_items(seeds: list[dict], per_page: int) -> list[dict]:
    items = []
    for seed in seeds:
        for n in range(per_page):
            items.append(
                {
                    "key": f"struct:{seed.get('document_id')}:"
                    f"{seed.get('page_number')}:{n}",
                    "kind": "structure",
                    "style": STYLES[n % len(STYLES)],
                    "seed": seed,
                }
            )
    return items


def cmd_structure(args: argparse.Namespace) -> int:
    out_dir = Path(args.out).expanduser()
    out_dir.mkdir(parents=True, exist_ok=True)
    out = out_dir / "structure_synth.jsonl"
    seeds = load_seeds(Path(args.seeds).expanduser())
    items = structure_items(seeds, args.per_page)
    done = scan_done_keys([out])
    clients = build_generator_clients(args, generator_urls(args.generator_url))

    def process(item: dict, client, endpoint: str, state: RunState) -> None:
        seed = item["seed"]
        prompt = STRUCT_PROMPT.format(
            style=item["style"],
            filename=seed.get("filename") or "(unknown)",
            page_number=seed.get("page_number") or "?",
            content=page_content(seed)[:6000],
        )
        text, usage = client.generate(prompt, args.max_tokens)
        text = text.strip()
        if word_count(text) < 60:
            log_failed_item(
                out_dir, item, "rejected: output under 60 words", endpoint
            )
            state.record_failure(rejected=True)
            return
        gen_tokens = usage.get("completion_tokens")
        if not isinstance(gen_tokens, int):
            gen_tokens = 0
        append_jsonl(
            out,
            {
                "key": item["key"],
                "text": text,
                "kind": "structure",
                "style": item["style"],
                "gen_tokens": gen_tokens,
                "generator": client.provenance,
                "source": {
                    "document_id": seed.get("document_id"),
                    "page_number": seed.get("page_number"),
                },
            },
        )
        state.record_success(item["kind"], usage)

    return run_parallel_items(items, done, out_dir, clients, args, process)


def vision_seed(seed: dict) -> bool:
    return bool((seed.get("visual_text") or "").strip())


def vision_items(seeds: list[dict], batch_pages: int) -> list[dict]:
    selected = [seed for seed in seeds if vision_seed(seed)]
    items = []
    for batch_index, start in enumerate(range(0, len(selected), batch_pages)):
        batch = selected[start : start + batch_pages]
        items.append(
            {
                "key": f"vision_batch:{batch_index:04d}",
                "kind": "vision_batch",
                "batch_index": batch_index,
                "seeds": batch,
            }
        )
    return items


def render_vision_record(index: int, seed: dict, max_chars: int) -> str:
    content = "\n\n".join(
        part
        for part in (
            seed.get("visual_text") or "",
            seed.get("tables_md") or "",
            seed.get("text") or "",
        )
        if part
    )
    topics = ", ".join(seed.get("summary_topics") or [])
    return (
        f"INDEX: {index}\n"
        f"DOCUMENT: {seed.get('filename') or '(unknown)'}\n"
        f"DOCUMENT_ID: {seed.get('document_id')}\n"
        f"PAGE: {seed.get('page_number')}\n"
        f"TOPICS: {topics or '(none)'}\n"
        f"VISION/OCR CONTENT:\n{content[:max_chars]}"
    )


def parsed_vision_outputs(raw: str, expected: int) -> dict[int, list[str]]:
    parsed = clean_json_array(raw)
    out: dict[int, list[str]] = {}
    for item in parsed:
        if not isinstance(item, dict):
            continue
        try:
            index = int(item.get("index"))
        except (TypeError, ValueError):
            continue
        passages = item.get("passages")
        if isinstance(passages, str):
            passages = [passages]
        if not isinstance(passages, list):
            continue
        clean_passages = [str(p).strip() for p in passages if str(p).strip()]
        if clean_passages:
            out[index] = clean_passages
    return {i: out.get(i, []) for i in range(expected)}


def cmd_vision(args: argparse.Namespace) -> int:
    out_dir = Path(args.out).expanduser()
    out_dir.mkdir(parents=True, exist_ok=True)
    out = out_dir / "vision_synth.jsonl"
    seeds = load_seeds(Path(args.seeds).expanduser())
    if args.batch_pages <= 0:
        eprint("--batch-pages must be > 0")
        return 2
    items = vision_items(seeds, args.batch_pages)
    done = scan_done_keys([out])
    clients = build_generator_clients(args, generator_urls(args.generator_url))

    def process(item: dict, client, endpoint: str, state: RunState) -> None:
        batch = item["seeds"]
        records = "\n\n---\n\n".join(
            render_vision_record(index, seed, args.page_chars)
            for index, seed in enumerate(batch)
        )
        prompt = VISION_PROMPT.format(
            per_page=args.per_page,
            records=records,
        )
        raw, usage = client.generate(prompt, args.max_tokens)
        try:
            by_index = parsed_vision_outputs(raw, len(batch))
        except (ValueError, json.JSONDecodeError):
            raw, retry_usage = client.generate(
                prompt + "\n\nReturn ONLY the JSON array, with all passages as strings.",
                args.max_tokens,
            )
            usage = merge_usage([usage, retry_usage])
            try:
                by_index = parsed_vision_outputs(raw, len(batch))
            except (ValueError, json.JSONDecodeError) as err:
                log_failed_item(out_dir, item, f"json parse failed: {err}", endpoint)
                state.record_failure()
                return

        written = 0
        rejected = 0
        gen_tokens = usage.get("completion_tokens")
        if not isinstance(gen_tokens, int):
            gen_tokens = 0
        for index, seed in enumerate(batch):
            passages = by_index.get(index, [])
            for passage_index, text in enumerate(passages[: args.per_page]):
                wc = word_count(text)
                if wc < args.min_words:
                    rejected += 1
                    continue
                append_jsonl(
                    out,
                    {
                        "key": (
                            f"vision:{seed.get('document_id')}:"
                            f"{seed.get('page_number')}:{passage_index}"
                        ),
                        "batch_key": item["key"],
                        "text": text,
                        "kind": "vision",
                        "style": STYLES[(item["batch_index"] + passage_index) % len(STYLES)],
                        "gen_tokens": gen_tokens // max(1, len(batch) * args.per_page),
                        "generator": client.provenance,
                        "source": {
                            "document_id": seed.get("document_id"),
                            "filename": seed.get("filename"),
                            "page_number": seed.get("page_number"),
                        },
                    },
                )
                written += 1
            if len(passages) < args.per_page:
                rejected += args.per_page - len(passages)
        if written == 0:
            log_failed_item(out_dir, item, "no vision passages survived filters", endpoint)
            state.record_failure(rejected=True)
            return
        state.record_success("vision", usage, records=written, rejected=rejected)

    return run_parallel_items(items, done, out_dir, clients, args, process)


def existing_question_hashes(path: Path) -> set[str]:
    seen = set()
    for record in load_jsonl(path):
        messages = record.get("messages") or []
        for message in messages:
            if message.get("role") == "user":
                seen.add(question_hash(message.get("content") or ""))
    return seen


def qa_items(seeds: list[dict]) -> list[dict]:
    return [
        {
            "key": f"qa:{seed.get('document_id')}:{seed.get('page_number')}",
            "kind": "qa",
            "seed": seed,
        }
        for seed in seeds
    ]


def cmd_qa(args: argparse.Namespace) -> int:
    out_dir = Path(args.out).expanduser()
    out_dir.mkdir(parents=True, exist_ok=True)
    out = out_dir / "sft_domain.jsonl"
    seeds = load_seeds(Path(args.seeds).expanduser(), args.limit_pages)
    items = qa_items(seeds)
    done = scan_done_keys([out])
    seen_questions = existing_question_hashes(out)
    seen_lock = threading.Lock()
    clients = build_generator_clients(args, generator_urls(args.generator_url))

    def process(item: dict, client, endpoint: str, state: RunState) -> None:
        seed = item["seed"]
        prompt = GEN_PROMPT.format(
            per_fact=args.per_fact,
            notes="\n".join(f"- {n}" for n in seed.get("summary_notes") or [])
            or "(none)",
            topics=", ".join(seed.get("summary_topics") or []) or "(none)",
            filename=seed.get("filename") or "(none)",
            page_number=seed.get("page_number") or "(none)",
            text=seed.get("text") or "(none)",
            tables_md=seed.get("tables_md") or "(none)",
            visual_text=seed.get("visual_text") or "(none)",
        )
        raw, usage = client.generate(prompt, args.max_tokens)
        try:
            pairs = clean_json_array(raw)
        except (ValueError, json.JSONDecodeError):
            raw, retry_usage = client.generate(
                prompt + "\n\nReturn ONLY the JSON array.",
                args.max_tokens,
            )
            usage = merge_usage([usage, retry_usage])
            try:
                pairs = clean_json_array(raw)
            except (ValueError, json.JSONDecodeError) as err:
                log_failed_item(out_dir, item, f"json parse failed: {err}", endpoint)
                state.record_failure()
                return
        kept = 0
        rejected = 0
        for pair in pairs:
            if not isinstance(pair, dict):
                rejected += 1
                continue
            question = str(pair.get("question") or "").strip()
            answer = str(pair.get("answer") or "").strip()
            q_hash = question_hash(question)
            if not question or word_count(answer) < 10:
                rejected += 1
                continue
            with seen_lock:
                if q_hash in seen_questions:
                    rejected += 1
                    continue
                seen_questions.add(q_hash)
            append_jsonl(
                out,
                {
                    "key": item["key"],
                    "messages": [
                        {"role": "system", "content": args.system},
                        {"role": "user", "content": question},
                        {"role": "assistant", "content": answer},
                    ],
                    "_source": {
                        "document_id": seed.get("document_id"),
                        "page_number": seed.get("page_number"),
                    },
                    "generator": client.provenance,
                },
            )
            kept += 1
        if kept == 0:
            log_failed_item(out_dir, item, "no QA pairs survived filters", endpoint)
            state.record_failure(rejected=True)
            return
        state.record_success("qa", usage, records=kept, rejected=rejected)

    return run_parallel_items(items, done, out_dir, clients, args, process)


def cmd_judge(args: argparse.Namespace) -> int:
    if args.batch_size <= 0:
        eprint("--batch-size must be > 0")
        return 2
    out_dir = Path(args.out).expanduser()
    out_dir.mkdir(parents=True, exist_ok=True)
    accepted_out = out_dir / "sft_judged.jsonl"
    rejected_out = out_dir / "sft_judge_rejected.jsonl"
    seeds = load_seeds(Path(args.seeds).expanduser())
    source_index = seed_content_index(seeds)
    sft_records = load_jsonl(Path(args.sft).expanduser())
    items = judge_items(sft_records, args.batch_size)
    done = scan_done_keys([accepted_out, rejected_out])
    clients = build_judge_clients(args)

    def process(item: dict, client, endpoint: str, state: RunState) -> None:
        source = source_index.get((item["document_id"], item["page_number"]), "")
        prompt = judge_prompt(source, item["records"])
        raw, usage = client.generate(prompt, args.max_tokens)
        try:
            scores = clean_json_array(raw)
        except (ValueError, json.JSONDecodeError):
            raw, retry_usage = client.generate(
                prompt + "\n\nReturn ONLY the JSON array.",
                args.max_tokens,
            )
            usage = merge_usage([usage, retry_usage])
            try:
                scores = clean_json_array(raw)
            except (ValueError, json.JSONDecodeError) as err:
                log_failed_item(out_dir, item, f"json parse failed: {err}", endpoint)
                state.record_failure()
                return

        scores_by_index = index_judge_scores(scores)
        accepted = 0
        rejected = 0
        for index, record in enumerate(item["records"], start=1):
            judged, is_accepted = judge_record(
                record,
                item["key"],
                scores_by_index.get(index),
                client.provenance,
                args.accept_min,
            )
            if is_accepted:
                append_jsonl(accepted_out, judged)
                accepted += 1
            else:
                append_jsonl(rejected_out, judged)
                rejected += 1
        state.record_result_counts(
            {
                "judged_accepted": accepted,
                "judged_rejected": rejected,
            },
            usage,
            rejected=rejected,
        )

    return run_parallel_items(items, done, out_dir, clients, args, process)


def probe_items(path: Path) -> list[dict]:
    items = []
    for record in load_jsonl(path):
        messages = record.get("messages") or []
        question = next(
            (m.get("content") for m in messages if m.get("role") == "user"),
            "",
        )
        answer = next(
            (m.get("content") for m in messages if m.get("role") == "assistant"),
            "",
        )
        source_key = record.get("key") or f"qa:{question_hash(question)}"
        key = f"probe:{source_key}:{question_hash(question)[:16]}"
        record = dict(record)
        source = dict(record.get("_source") or {})
        source["qa_key"] = source_key
        record["_source"] = source
        record["key"] = key
        items.append(
            {
                "key": key,
                "kind": "probe",
                "record": record,
                "question": question,
                "answer": answer,
            }
        )
    return items


def cmd_probe(args: argparse.Namespace) -> int:
    out_dir = Path(args.out).expanduser()
    out_dir.mkdir(parents=True, exist_ok=True)
    known_out = out_dir / "sft_known.jsonl"
    unknown_out = out_dir / "sft_unknown.jsonl"
    items = probe_items(Path(args.sft).expanduser())
    done = scan_done_keys([known_out, unknown_out])
    clients = build_generator_clients(args, generator_urls(args.base_url))

    def process(item: dict, client, endpoint: str, state: RunState) -> None:
        usages = []
        passes = 0
        for _ in range(3):
            raw, usage = client.generate(item["question"], args.max_tokens)
            usages.append(usage)
            if token_overlap(item["answer"], raw) >= args.threshold:
                passes += 1
        record = item["record"]
        if passes >= 2:
            append_jsonl(known_out, record)
            state.record_success("probe_known", merge_usage(usages))
        else:
            append_jsonl(unknown_out, record)
            state.record_success("probe_unknown", merge_usage(usages))

    return run_parallel_items(items, done, out_dir, clients, args, process)


def scrub_training_record(record: dict) -> dict:
    clean = dict(record)
    clean.pop("_source", None)
    clean.pop("key", None)
    return clean


def cmd_mix(args: argparse.Namespace) -> int:
    out_dir = Path(args.out).expanduser()
    out_dir.mkdir(parents=True, exist_ok=True)
    sft = load_jsonl(Path(args.sft).expanduser())
    general = load_jsonl(Path(args.general).expanduser())
    cpt = load_jsonl(Path(args.cpt).expanduser())
    rng = random.Random(13)
    if not 0 <= args.sft_general_ratio < 1:
        eprint("--sft-general-ratio must be >=0 and <1")
        return 2
    n_general = min(
        len(general),
        int(len(sft) * args.sft_general_ratio / (1 - args.sft_general_ratio)),
    )
    general_sample = rng.sample(general, n_general) if n_general else []
    docs = sorted(
        {
            r.get("_source", {}).get("document_id")
            for r in sft
            if r.get("_source", {}).get("document_id") is not None
        }
    )
    rng.shuffle(docs)
    val_docs = set(docs[int(len(docs) * args.split) :])
    train_sft = [scrub_training_record(r) for r in general_sample]
    val = []
    for record in sft:
        target = val if record.get("_source", {}).get("document_id") in val_docs else train_sft
        target.append(scrub_training_record(record))
    rng.shuffle(train_sft)
    n_replay = int(len(cpt) * args.cpt_replay_ratio)
    replay = [
        {"text": (r.get("messages") or [{}])[-1].get("content", "")}
        for r in rng.sample(general_sample, min(n_replay, len(general_sample)))
    ]
    train_cpt = [scrub_training_record(r) for r in cpt] + [
        r for r in replay if r["text"]
    ]
    rng.shuffle(train_cpt)
    for name, rows in (
        ("train_sft.jsonl", train_sft),
        ("val.jsonl", val),
        ("train_cpt.jsonl", train_cpt),
    ):
        with (out_dir / name).open("w") as f:
            for row in rows:
                f.write(json.dumps(row) + "\n")
    (out_dir / "valid.jsonl").write_text((out_dir / "val.jsonl").read_text())
    eprint(
        f"mix: sft={len(train_sft)} ({n_general} general), "
        f"cpt={len(train_cpt)}, val={len(val)} across {len(val_docs)} docs "
        "(+valid.jsonl alias)"
    )
    eprint("NOW: SemHash-decontaminate val + your eval suite against both train files.")
    return 0


def add_generation_flags(parser: argparse.ArgumentParser, default_temperature: float) -> None:
    parser.add_argument(
        "--generator-url",
        action="append",
        help="OpenAI-compatible base URL; repeatable (default: LAN llama.cpp pool)",
    )
    parser.add_argument("--temperature", type=float, default=default_temperature)
    parser.add_argument("--top-p", type=float, default=0.95)
    parser.add_argument("--top-k", type=int, default=64)
    parser.add_argument(
        "--codex-workers",
        type=int,
        default=0,
        help="parallel codex exec workers to add to the generator pool",
    )
    parser.add_argument(
        "--codex-effort",
        choices=("low", "medium", "high"),
        default="low",
        help="codex exec model_reasoning_effort value",
    )
    parser.add_argument(
        "--codex-cmd",
        default="codex",
        help="codex CLI binary override",
    )


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    sub = parser.add_subparsers(dest="command", required=True)
    seeds = sub.add_parser("seeds", help="compile the merged seeds.jsonl corpus file")
    seeds.add_argument(
        "--inputs",
        action="append",
        required=True,
        help="glob(s) of summarizer *_output.json files (repeatable)",
    )
    seeds.add_argument("--out", default="data", help="output directory")
    seeds.add_argument(
        "--min-relevancy",
        type=int,
        default=DEFAULT_MIN_RELEVANCY,
        help="drop pages whose summary_relevancy is below this (default 70)",
    )
    seeds.add_argument(
        "--keep-warned",
        action="store_true",
        help="keep pages that carry extraction_warnings (default: drop)",
    )
    seeds.add_argument(
        "--catalog",
        default=str(DEFAULT_CATALOG),
        help="cli-runs.jsonl catalog for source content-hash joins",
    )
    seeds.set_defaults(func=cmd_seeds)

    entigraph = sub.add_parser("entigraph", help="generate synthetic CPT corpus")
    entigraph.add_argument("--seeds", required=True)
    entigraph.add_argument("--out", required=True)
    add_generation_flags(entigraph, 1.0)
    entigraph.add_argument("--pairs", type=int, default=4000)
    entigraph.add_argument("--triples", type=int, default=1000)
    entigraph.add_argument("--rephrasings", type=int, default=8)
    entigraph.add_argument("--max-tokens", type=int, default=700)
    entigraph.set_defaults(func=cmd_entigraph)

    structure = sub.add_parser(
        "structure",
        help="generate layout/geometry CPT passages (single-page, F1 lane)",
    )
    structure.add_argument("--seeds", required=True)
    structure.add_argument("--out", required=True)
    add_generation_flags(structure, 1.0)
    structure.add_argument("--per-page", type=int, default=24)
    structure.add_argument("--max-tokens", type=int, default=600)
    structure.set_defaults(func=cmd_structure)

    vision = sub.add_parser(
        "vision",
        help="generate page-scoped vision/OCR CPT passages over image_text content",
    )
    vision.add_argument("--seeds", required=True)
    vision.add_argument("--out", required=True)
    add_generation_flags(vision, 0.8)
    vision.add_argument("--per-page", type=int, default=4)
    vision.add_argument("--batch-pages", type=int, default=4)
    vision.add_argument("--page-chars", type=int, default=5000)
    vision.add_argument("--min-words", type=int, default=55)
    vision.add_argument("--max-tokens", type=int, default=5000)
    vision.set_defaults(func=cmd_vision)

    qa = sub.add_parser("qa", help="generate grounded Q&A chat records")
    qa.add_argument("--seeds", required=True)
    qa.add_argument("--out", required=True)
    add_generation_flags(qa, 1.0)
    qa.add_argument("--per-fact", type=int, default=10)
    qa.add_argument("--max-tokens", type=int, default=4000)
    qa.add_argument("--system", default=DEFAULT_QA_SYSTEM)
    qa.add_argument("--limit-pages", type=int)
    qa.set_defaults(func=cmd_qa)

    judge = sub.add_parser("judge", help="judge grounded Q&A records")
    judge.add_argument("--sft", required=True)
    judge.add_argument("--seeds", required=True)
    judge.add_argument("--out", required=True)
    judge.add_argument(
        "--generator-url",
        action="append",
        help="optional OpenAI-compatible judge URL; no HTTP judge by default",
    )
    judge.add_argument("--temperature", type=float, default=1.0)
    judge.add_argument("--top-p", type=float, default=0.95)
    judge.add_argument("--top-k", type=int, default=64)
    judge.add_argument("--codex-workers", type=int, default=4)
    judge.add_argument(
        "--codex-effort",
        choices=("low", "medium", "high"),
        default="low",
    )
    judge.add_argument("--codex-cmd", default="codex")
    judge.add_argument("--batch-size", type=int, default=15)
    judge.add_argument("--accept-min", type=int, default=7)
    judge.add_argument("--max-tokens", type=int, default=2000)
    judge.set_defaults(func=cmd_judge)

    probe = sub.add_parser("probe", help="split Q&A through a closed-book base probe")
    probe.add_argument("--sft", required=True)
    probe.add_argument("--out", required=True)
    probe.add_argument(
        "--base-url",
        action="append",
        help="OpenAI-compatible base URL; repeatable (default: LAN llama.cpp pool)",
    )
    probe.add_argument("--temperature", type=float, default=0.7)
    probe.add_argument("--top-p", type=float, default=0.95)
    probe.add_argument("--top-k", type=int, default=64)
    probe.add_argument("--max-tokens", type=int, default=512)
    probe.add_argument("--threshold", type=float, default=0.5)
    probe.set_defaults(func=cmd_probe)

    mix = sub.add_parser("mix", help="mix SFT/CPT/general data and split validation")
    mix.add_argument("--sft", required=True)
    mix.add_argument("--cpt", required=True)
    mix.add_argument("--general", required=True)
    mix.add_argument("--out", required=True)
    mix.add_argument("--sft-general-ratio", type=float, default=0.33)
    mix.add_argument("--cpt-replay-ratio", type=float, default=0.10)
    mix.add_argument("--split", type=float, default=0.95)
    mix.set_defaults(func=cmd_mix)

    args = parser.parse_args()
    return args.func(args)


if __name__ == "__main__":
    raise SystemExit(main())
