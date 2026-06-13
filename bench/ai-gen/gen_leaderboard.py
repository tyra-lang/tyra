#!/usr/bin/env python3
"""Generate LEADERBOARD.md from ai-gen manifest files and hardcoded historical data.

Usage:
    python3 bench/ai-gen/gen_leaderboard.py > bench/ai-gen/LEADERBOARD.md
    python3 bench/ai-gen/gen_leaderboard.py --check   # verify data only

Historical data for other languages comes from prior sweep runs documented in INSIGHTS.md.
Tyra data is computed live from the latest multi-seed manifest (results-run56-manifest.csv).
"""

import csv
import sys
from pathlib import Path

HERE = Path(__file__).parent

# ---------------------------------------------------------------------------
# Historical data (from INSIGHTS.md / prior sweep runs)
# Single-seed seed-1 results unless noted.
# ---------------------------------------------------------------------------
HISTORICAL = [
    {
        "language": "ruby",
        "display": "Ruby",
        "pass_pct": 99.0,
        "fail_pct": 0.0,
        "runs": 100,
        "notes": "seed-1",
        "condition": "single-seed",
    },
    {
        "language": "crystal",
        "display": "Crystal",
        "pass_pct": 96.0,
        "fail_pct": 3.0,
        "runs": 100,
        "notes": "seed-1",
        "condition": "single-seed",
    },
    {
        "language": "go",
        "display": "Go",
        "pass_pct": 81.0,
        "fail_pct": 16.0,
        "runs": 100,
        "notes": "seed-1",
        "condition": "single-seed",
    },
    {
        "language": "v",
        "display": "V",
        "pass_pct": 49.0,
        "fail_pct": 50.0,
        "runs": 100,
        "notes": "seed-1",
        "condition": "single-seed",
    },
    {
        "language": "gleam",
        "display": "Gleam",
        "pass_pct": 37.0,
        "fail_pct": 57.0,
        "runs": 100,
        "notes": "seed-1; single-file wrapper pessimism",
        "condition": "single-seed",
    },
]

# ---------------------------------------------------------------------------
# Compute Tyra stats from run56 manifest
# ---------------------------------------------------------------------------

def compute_tyra(manifest_path: Path) -> dict:
    rows = []
    with open(manifest_path) as f:
        for r in csv.DictReader(f):
            rows.append(r)

    outcome_col = "overall" if "overall" in rows[0] else "outcome"
    by_prompt: dict[str, list[str]] = {}
    for r in rows:
        prompt = r.get("prompt") or r["stem"].split("__")[0]
        by_prompt.setdefault(prompt, []).append(r[outcome_col])

    total_runs = len(rows)
    pass_runs = sum(1 for r in rows if r[outcome_col] == "pass")
    n_prompts = len(by_prompt)
    any_pass = sum(1 for v in by_prompt.values() if "pass" in v)
    all_pass = sum(1 for v in by_prompt.values() if all(x == "pass" for x in v))
    fail_all = [p for p, v in by_prompt.items() if "pass" not in v]

    seeds = sorted({r["stem"].rsplit("__s", 1)[-1] for r in rows if "__s" in r["stem"]})
    n_seeds = len(seeds)

    return {
        "language": "tyra+spec",
        "display": "Tyra + spec",
        "pass_pct": pass_runs / total_runs * 100,
        "fail_pct": (total_runs - pass_runs) / total_runs * 100,
        "runs": total_runs,
        "notes": f"{n_seeds} seeds × {n_prompts} prompts; any_pass {any_pass/n_prompts*100:.0f}%, all_pass {all_pass/n_prompts*100:.0f}%",
        "condition": f"multi-seed ({n_seeds})",
        "any_pass_pct": any_pass / n_prompts * 100,
        "all_pass_pct": all_pass / n_prompts * 100,
        "fail_all_prompts": fail_all,
        "n_seeds": n_seeds,
        "n_prompts": n_prompts,
        "pass_runs": pass_runs,
    }


# ---------------------------------------------------------------------------
# Render
# ---------------------------------------------------------------------------

HEADER = """\
# AI Code Generation Leaderboard

How well does Claude generate correct Tyra code on first try, given the language spec?

**Benchmark**: 100 programming tasks from simple to moderate difficulty.
**Generator**: Claude (claude-sonnet), no human feedback, first attempt only.
**Pass criterion**: program compiles and produces the expected output on all test cases.

> **Methodology note**: Tyra is swept with 3 seeds (300 runs total); other languages
> use seed-1 only. Multi-seed mean is a more stable estimate but is not directly
> comparable to seed-1 point estimates. See [METHODOLOGY.md](METHODOLOGY.md) for details.

## Results

"""

TABLE_HEADER = (
    "| Language | Pass rate | Runs | Condition | Notes |\n"
    "|----------|-----------|------|-----------|-------|\n"
)

FOOTER_TEMPLATE = """\

† seed-1 only; multi-seed comparison pending.

## Tyra detail (v0.11.0, run56)

- **Mean pass rate**: {mean:.1f}% ({pass_runs}/{total_runs} runs)
- **Any-seed pass**: {any_pass:.0f}% of prompts pass on ≥1 seed
- **All-seed pass**: {all_pass:.0f}% of prompts pass on all {n_seeds} seeds
- **Fail-all prompts** ({n_fail}): {fail_list}

## Caveats

- Ruby and Crystal figures are from prior sweeps (same generator, seed-1).
- Go, V, and Gleam are seed-1 only; multi-seed results pending.
- "Tyra + spec" injects `llms-full.md` into the system prompt.
  "Tyra (no spec)" scores 0% — the model has no Tyra training data.
- Gleam pass rate is pessimistic: the runner wraps generated code in a project
  template; models that produce only a function body are counted as failures.

## Reproducing

```sh
# Regenerate this file (committed manifest, no re-run needed):
python3 bench/ai-gen/gen_leaderboard.py

# Re-run the sweep from scratch:
python3 bench/ai-gen/harness.py --prompts bench/ai-gen/prompts/ \\
  --language tyra+spec --seeds 1 2 3 --out bench/ai-gen/results-run56/
```

*Generated by `bench/ai-gen/gen_leaderboard.py` from `results-run56-manifest.csv`.*
"""


def render(tyra: dict) -> str:
    entries = HISTORICAL + [tyra]
    entries.sort(key=lambda e: -e["pass_pct"])

    lines = [HEADER, TABLE_HEADER]
    for e in entries:
        cond_marker = " †" if e["condition"].startswith("single-seed") else ""
        lines.append(
            f"| **{e['display']}** | **{e['pass_pct']:.1f}%** | {e['runs']} "
            f"| {e['condition']}{cond_marker} | {e['notes']} |\n"
        )

    fail_list = ", ".join(f"`{p}`" for p in tyra["fail_all_prompts"])
    lines.append(
        FOOTER_TEMPLATE.format(
            mean=tyra["pass_pct"],
            pass_runs=tyra["pass_runs"],
            total_runs=tyra["runs"],
            any_pass=tyra["any_pass_pct"],
            all_pass=tyra["all_pass_pct"],
            n_seeds=tyra["n_seeds"],
            n_fail=len(tyra["fail_all_prompts"]),
            fail_list=fail_list or "none",
        )
    )

    return "".join(lines)


def main():
    manifest = HERE / "results-run56-manifest.csv"
    if not manifest.exists():
        print(f"error: {manifest} not found", file=sys.stderr)
        sys.exit(1)

    tyra = compute_tyra(manifest)

    out = render(tyra)
    outpath = HERE / "LEADERBOARD.md"

    if "--check" in sys.argv:
        if not outpath.exists():
            print(f"error: {outpath} does not exist — run without --check to generate", file=sys.stderr)
            sys.exit(1)
        current = outpath.read_text()
        if current == out:
            print(f"ok: {outpath} is up to date", file=sys.stderr)
        else:
            import difflib
            diff = list(difflib.unified_diff(
                current.splitlines(keepends=True),
                out.splitlines(keepends=True),
                fromfile="LEADERBOARD.md (on disk)",
                tofile="LEADERBOARD.md (expected)",
            ))
            print("error: LEADERBOARD.md is stale — run gen_leaderboard.py to regenerate", file=sys.stderr)
            sys.stderr.writelines(diff[:40])
            sys.exit(1)
        return

    outpath.write_text(out)
    print(f"wrote {outpath}", file=sys.stderr)


if __name__ == "__main__":
    main()
