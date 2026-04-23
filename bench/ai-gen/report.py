#!/usr/bin/env python3
"""bench/ai-gen/report.py — aggregate results/*.json into Markdown + CSV.

Prints Markdown to stdout by default. With --csv <path>, also writes a
CSV breakdown.
"""
from __future__ import annotations

import argparse
import csv
import json
import statistics
from collections import defaultdict
from pathlib import Path
from typing import Any, Dict, List, Tuple

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


def aggregate_by_seed(
    results: List[Dict[str, Any]],
) -> Dict[tuple, Dict[str, float]]:
    """Per (lang, generator) seed-aware stats.

    For each (prompt_id, lang, generator), collect the pass/fail outcome
    per seed. Then for each (lang, generator), roll up across prompts:

      - mean_pass: average over prompts of (passes / seeds_for_that_prompt).
        Equivalent to total passes / total runs when every prompt has the
        same number of seeds.
      - median_pass: median of the per-prompt pass fractions. Robust to
        a few hard prompts dragging the mean down.
      - any_pass: fraction of prompts where at least one seed passed.
        Upper bound on "could the model solve this if we resampled".
      - all_pass: fraction of prompts where every seed passed. Lower
        bound / stability indicator.
      - seeds_per_prompt: min..max observed, to sanity-check the sweep.
    """
    # (lang, gen) -> prompt_id -> list of bool (True if outcome == "pass")
    grid: Dict[tuple, Dict[str, List[bool]]] = defaultdict(
        lambda: defaultdict(list)
    )
    for r in results:
        lang = r.get("language", "?")
        if r.get("inject_tyra_spec"):
            lang = f"{lang}+spec"
        if r.get("overall") == "skipped_no_compiler":
            continue
        key = (lang, r.get("generator", "?"))
        pid = r.get("prompt_id", "?")
        grid[key][pid].append(r.get("overall") == "pass")

    out: Dict[tuple, Dict[str, float]] = {}
    for key, per_prompt in grid.items():
        pass_fracs: List[float] = []
        any_count = 0
        all_count = 0
        seed_counts: List[int] = []
        for pid, outcomes in per_prompt.items():
            n = len(outcomes)
            seed_counts.append(n)
            passes = sum(1 for o in outcomes if o)
            pass_fracs.append(passes / n if n else 0.0)
            if passes >= 1:
                any_count += 1
            if passes == n and n > 0:
                all_count += 1
        n_prompts = len(per_prompt)
        out[key] = {
            "n_prompts": n_prompts,
            "mean_pass": (
                statistics.fmean(pass_fracs) if pass_fracs else 0.0
            ),
            "median_pass": (
                statistics.median(pass_fracs) if pass_fracs else 0.0
            ),
            "any_pass": (any_count / n_prompts) if n_prompts else 0.0,
            "all_pass": (all_count / n_prompts) if n_prompts else 0.0,
            "seeds_min": min(seed_counts) if seed_counts else 0,
            "seeds_max": max(seed_counts) if seed_counts else 0,
        }
    return out


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


def render_seed_markdown(
    seed_stats: Dict[tuple, Dict[str, float]],
) -> str:
    if not seed_stats:
        return ""
    lines: List[str] = []
    lines.append("## Multi-seed aggregates")
    lines.append("")
    lines.append(
        "Rolled up per (language, generator) by first computing the "
        "per-prompt pass fraction across seeds, then summarising across "
        "prompts. `any_pass` = ≥1 seed passed; `all_pass` = every seed "
        "passed. `seeds` column shows the observed min/max seed count "
        "per prompt — they should match unless a sweep was partial."
    )
    lines.append("")
    lines.append(
        "| language | generator | prompts | seeds | mean_pass% | "
        "median_pass% | any_pass% | all_pass% |"
    )
    lines.append(
        "| -------- | --------- | ------- | ----- | ---------- | "
        "------------ | --------- | --------- |"
    )
    for key in sorted(seed_stats.keys()):
        lang, gen = key
        s = seed_stats[key]
        seed_col = (
            f"{s['seeds_min']}"
            if s["seeds_min"] == s["seeds_max"]
            else f"{s['seeds_min']}..{s['seeds_max']}"
        )
        lines.append(
            f"| {lang} | {gen} | {int(s['n_prompts'])} | {seed_col} | "
            f"{s['mean_pass']*100:.1f}% | {s['median_pass']*100:.1f}% | "
            f"{s['any_pass']*100:.1f}% | {s['all_pass']*100:.1f}% |"
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
    seed_stats = aggregate_by_seed(results)
    # Only print the seed section if at least one (lang, gen) was run
    # with ≥2 seeds — otherwise it is redundant with the pass% column.
    if any(s["seeds_max"] >= 2 for s in seed_stats.values()):
        print(render_seed_markdown(seed_stats))
    if args.csv:
        write_csv(Path(args.csv), buckets)
    return 0


if __name__ == "__main__":
    import sys

    sys.exit(main())
