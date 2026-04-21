#!/usr/bin/env python3
"""bench/ai-gen/report.py — aggregate results/*.json into Markdown + CSV.

Prints Markdown to stdout by default. With --csv <path>, also writes a
CSV breakdown.
"""
from __future__ import annotations

import argparse
import csv
import json
from collections import defaultdict
from pathlib import Path
from typing import Any, Dict, List

HERE = Path(__file__).resolve().parent

OUTCOMES = [
    "pass",
    "check_fail",
    "exec_fail",
    "compile_fail",
    "generator_fail",
    "harness_error",
    "skipped_no_compiler",
]


def load_results(results_dir: Path) -> List[Dict[str, Any]]:
    out = []
    for p in sorted(results_dir.glob("*.json")):
        with open(p) as f:
            out.append(json.load(f))
    return out


def aggregate(
    results: List[Dict[str, Any]],
) -> Dict[tuple, Dict[str, int]]:
    buckets: Dict[tuple, Dict[str, int]] = defaultdict(
        lambda: {o: 0 for o in OUTCOMES}
    )
    for r in results:
        # Show spec-injected runs on their own row so the baseline is
        # never averaged together with the RAG experiment.
        lang = r.get("language", "?")
        if r.get("inject_tyra_spec"):
            lang = f"{lang}+spec"
        key = (lang, r.get("generator", "?"))
        outcome = r.get("overall", "harness_error")
        buckets[key][outcome] = buckets[key].get(outcome, 0) + 1
    return buckets


def render_markdown(
    buckets: Dict[tuple, Dict[str, int]], total_prompts: int
) -> str:
    lines: List[str] = []
    lines.append("# ai-gen benchmark summary")
    lines.append("")
    lines.append(f"Prompts observed: {total_prompts}")
    lines.append("")
    lines.append(
        "| language | generator | pass | check_fail | exec_fail | "
        "compile_fail | generator_fail | harness_error | skipped | "
        "total | pass% |"
    )
    lines.append(
        "| -------- | --------- | ---- | ---------- | --------- | "
        "------------ | -------------- | ------------- | ------- | "
        "----- | ----- |"
    )
    for key in sorted(buckets.keys()):
        lang, gen = key
        b = buckets[key]
        total = sum(b.values())
        non_skip = total - b["skipped_no_compiler"]
        pass_pct = (b["pass"] / non_skip * 100.0) if non_skip else 0.0
        lines.append(
            f"| {lang} | {gen} | {b['pass']} | {b['check_fail']} | "
            f"{b['exec_fail']} | {b['compile_fail']} | "
            f"{b['generator_fail']} | {b['harness_error']} | "
            f"{b['skipped_no_compiler']} | {total} | {pass_pct:.1f}% |"
        )
    lines.append("")
    lines.append(
        "`pass%` is computed against non-skipped runs so a missing "
        "compiler does not depress the headline number."
    )
    return "\n".join(lines) + "\n"


def write_csv(
    path: Path, buckets: Dict[tuple, Dict[str, int]]
) -> None:
    with open(path, "w", newline="") as f:
        w = csv.writer(f)
        w.writerow(["language", "generator", *OUTCOMES, "total", "pass_rate"])
        for key in sorted(buckets.keys()):
            lang, gen = key
            b = buckets[key]
            total = sum(b.values())
            non_skip = total - b["skipped_no_compiler"]
            pass_rate = (b["pass"] / non_skip) if non_skip else 0.0
            w.writerow(
                [
                    lang,
                    gen,
                    *[b[o] for o in OUTCOMES],
                    total,
                    f"{pass_rate:.4f}",
                ]
            )


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--results-dir", default=str(HERE / "results"))
    ap.add_argument("--csv", default=None)
    args = ap.parse_args()

    results = load_results(Path(args.results_dir))
    prompts = {r.get("prompt_id") for r in results if "prompt_id" in r}
    buckets = aggregate(results)
    print(render_markdown(buckets, len(prompts)))
    if args.csv:
        write_csv(Path(args.csv), buckets)
    return 0


if __name__ == "__main__":
    import sys

    sys.exit(main())
