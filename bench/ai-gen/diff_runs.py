#!/usr/bin/env python3
"""bench/ai-gen/diff_runs.py — per-stem outcome diff between two result dirs.

The comparison key is the JSON filename stem, which encodes
  {prompt_id}__{lang}{spec_suffix}__{generator}__s{seed}
and is therefore collision-free across all (language, generator, seed) axes.

Inputs: each side (--baseline / --candidate) can be EITHER:
  - a directory containing *.json result files (full run dir), OR
  - a *.csv manifest file with columns "stem,outcome" (compact archive).

Usage:
    python3 diff_runs.py --baseline results-run51 --candidate results-run53
    python3 diff_runs.py --baseline results-run51 \
        --candidate results-run53-manifest.csv --filter tyra+spec
"""
from __future__ import annotations

import argparse
import csv
import json
from collections import defaultdict
from pathlib import Path
from typing import Dict, List, Tuple

HERE = Path(__file__).resolve().parent


def load_stems(source: Path, stem_filter: str | None) -> Dict[str, str]:
    """Return {stem: overall} from a results dir or a manifest CSV."""
    out: Dict[str, str] = {}
    if source.is_file() and source.suffix == ".csv":
        with open(source, newline="") as f:
            for row in csv.DictReader(f):
                stem = row["stem"]
                if stem_filter and stem_filter not in stem:
                    continue
                out[stem] = row["outcome"]
        return out
    for p in sorted(source.glob("*.json")):
        stem = p.stem
        if stem_filter and stem_filter not in stem:
            continue
        try:
            with open(p) as f:
                d = json.load(f)
            out[stem] = d.get("overall", "harness_error")
        except Exception:
            out[stem] = "harness_error"
    return out


def diff(
    baseline: Dict[str, str], candidate: Dict[str, str]
) -> Tuple[
    Dict[Tuple[str, str], List[str]],
    List[str],
    List[str],
]:
    """Compare two stem→outcome dicts.

    Returns:
        transitions: {(b_outcome, c_outcome): [stem, ...]}
        only_baseline: stems absent from candidate
        only_candidate: stems absent from baseline
    """
    all_keys = sorted(set(baseline) | set(candidate))
    transitions: Dict[Tuple[str, str], List[str]] = defaultdict(list)
    only_baseline: List[str] = []
    only_candidate: List[str] = []

    for stem in all_keys:
        if stem not in candidate:
            only_baseline.append(stem)
        elif stem not in baseline:
            only_candidate.append(stem)
        else:
            transitions[(baseline[stem], candidate[stem])].append(stem)

    return transitions, only_baseline, only_candidate


def render(
    transitions: Dict[Tuple[str, str], List[str]],
    only_baseline: List[str],
    only_candidate: List[str],
    baseline_dir: str,
    candidate_dir: str,
) -> str:
    lines: List[str] = []
    lines.append(f"## Outcome diff: `{baseline_dir}` → `{candidate_dir}`")
    lines.append("")

    # Summary table.
    lines.append("### Transition counts")
    lines.append("")
    lines.append("| baseline | candidate | count |")
    lines.append("| -------- | --------- | ----- |")
    for (b, c), stems in sorted(transitions.items(), key=lambda x: -len(x[1])):
        lines.append(f"| {b} | {c} | {len(stems)} |")
    lines.append("")

    # Changed (b != c) grouped by transition.
    changed = {k: v for k, v in transitions.items() if k[0] != k[1]}
    if changed:
        lines.append("### Changed runs")
        lines.append("")
        for (b, c), stems in sorted(changed.items(), key=lambda x: -len(x[1])):
            lines.append(f"#### `{b}` → `{c}` ({len(stems)})")
            lines.append("")
            for s in stems:
                lines.append(f"- `{s}`")
            lines.append("")
    else:
        lines.append("*No changed runs.*")
        lines.append("")

    # Missing.
    if only_baseline:
        lines.append(f"### Only in baseline ({len(only_baseline)} runs)")
        lines.append("")
        for s in only_baseline:
            lines.append(f"- `{s}`")
        lines.append("")
    if only_candidate:
        lines.append(f"### Only in candidate ({len(only_candidate)} runs)")
        lines.append("")
        for s in only_candidate:
            lines.append(f"- `{s}`")
        lines.append("")

    return "\n".join(lines) + "\n"


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--baseline", default=str(HERE / "results-run51"))
    ap.add_argument("--candidate", default=str(HERE / "results-run53"))
    ap.add_argument(
        "--filter",
        default=None,
        help="Only include stems containing this substring (e.g. 'tyra+spec').",
    )
    args = ap.parse_args()

    b_dir = Path(args.baseline)
    c_dir = Path(args.candidate)
    baseline = load_stems(b_dir, args.filter)
    candidate = load_stems(c_dir, args.filter)

    if not baseline:
        print(f"warning: no matching entries in {args.baseline}")
    if not candidate:
        print(f"warning: no matching entries in {args.candidate}")

    transitions, only_b, only_c = diff(baseline, candidate)
    print(render(transitions, only_b, only_c, args.baseline, args.candidate))
    return 0


if __name__ == "__main__":
    import sys

    sys.exit(main())
