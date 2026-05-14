#!/usr/bin/env python3
"""bench/ai-gen/replay.py — Replay cached model code through the current compiler.

Reads result JSONs from a previous harness run (which contain the model-generated
code) and re-runs only the compile + execute pipeline against the current compiler.
The LLM is NOT called; this isolates the effect of compiler fixes.

Usage:
    python3 replay.py --from results-run51 --to results-run52-replay
                      [--prompts 'prompts/*.yaml']
"""
from __future__ import annotations

import argparse
import collections
import json
import os
import sys
import time
from pathlib import Path
from typing import Any, Dict, List, Optional

HERE = Path(__file__).resolve().parent
REPO_ROOT = HERE.parent.parent
sys.path.insert(0, str(HERE))

from harness import decide_overall, load_config, load_prompts  # noqa: E402
from runners.tyra import TyraRunner  # noqa: E402


def replay_one(
    src: Dict[str, Any],
    prompt: Dict[str, Any],
    runner: TyraRunner,
    config: Dict[str, Any],
    src_filename: str,
) -> Dict[str, Any]:
    code: str = src["code"]
    result: Dict[str, Any] = {
        "prompt_id": src["prompt_id"],
        "language": src["language"],
        "generator": src["generator"],
        "seed": src["seed"],
        "timestamp": int(time.time()),
        # Carry inject_tyra_spec so report.py aggregates under the same row
        # (tyra+spec) as the original run.
        "inject_tyra_spec": src.get("inject_tyra_spec", False),
        "model": src.get("model"),
        "code": code,
        # Extra fields for auditing the replay
        "replayed_from": src_filename,
        "original_overall": src.get("overall"),
    }

    workdir = runner.prepare_workdir(code)
    try:
        compile_res = runner.compile(workdir)
        result["stages"] = {
            "compile": {
                "ok": compile_res.ok,
                "duration_ms": compile_res.duration_ms,
                "stderr": compile_res.stderr[-2000:],
                "exit_code": compile_res.exit_code,
                "note": compile_res.note,
            }
        }
        if not compile_res.ok:
            result["overall"] = "compile_fail"
            return result

        exec_timeout = prompt.get("execution", {}).get(
            "timeout_seconds",
            config["timeouts"]["execute_seconds_default"],
        )
        exec_res = runner.execute(
            workdir,
            prompt.get("execution", {}).get("stdin", ""),
            exec_timeout,
        )
        must_contain: List[str] = prompt.get("execution", {}).get(
            "stdout_must_contain", []
        )
        must_not_contain: List[str] = prompt.get("execution", {}).get(
            "stdout_must_not_contain", []
        )
        checks_passed, check_details = runner.evaluate_checks(
            exec_res.stdout, must_contain, must_not_contain
        )
        result["stages"]["execute"] = {
            "ok": exec_res.ok,
            "duration_ms": exec_res.duration_ms,
            "stdout": exec_res.stdout[-2000:],
            "stderr": exec_res.stderr[-2000:],
            "exit_code": exec_res.exit_code,
            "note": exec_res.note,
            "checks_passed": checks_passed,
            "check_details": check_details,
        }
        result["overall"] = decide_overall(
            gen_ok=True,
            compile_ok=True,
            execute_ok=exec_res.ok,
            checks_passed=checks_passed,
            skipped_compiler=False,
        )
        return result
    finally:
        runner.cleanup(workdir)


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument("--from", dest="from_dir", required=True,
                    help="Source results directory (e.g. results-run51)")
    ap.add_argument("--to", dest="to_dir", required=True,
                    help="Output results directory (e.g. results-run52-replay)")
    ap.add_argument("--prompts", default="prompts/*.yaml",
                    help="Glob for prompt YAML files")
    args = ap.parse_args()

    config = load_config()
    prompts_list = load_prompts([args.prompts])
    prompt_index: Dict[str, Any] = {p["id"]: p for p in prompts_list}

    from_dir = Path(args.from_dir)
    if not from_dir.is_absolute():
        from_dir = HERE / from_dir
    to_dir = Path(args.to_dir)
    if not to_dir.is_absolute():
        to_dir = HERE / to_dir
    to_dir.mkdir(parents=True, exist_ok=True)

    src_files = sorted(from_dir.glob("*.json"))
    if not src_files:
        print(f"error: no JSON files found in {from_dir}", file=sys.stderr)
        return 1

    runner = TyraRunner(config=config, repo_root=REPO_ROOT)
    if not runner.compiler_available():
        print(
            f"error: compiler not found at {REPO_ROOT / 'target' / 'debug' / 'tyra'}\n"
            "Run: cargo build -p tyra-cli",
            file=sys.stderr,
        )
        return 1

    transitions: collections.Counter = collections.Counter()
    n = len(src_files)

    for i, src_path in enumerate(src_files, 1):
        src = json.loads(src_path.read_text())
        lang = src.get("language", "")
        code = src.get("code")

        if lang != "tyra" or not code:
            print(f"[{i}/{n}] {src_path.name}  SKIP (lang={lang!r}, has_code={bool(code)})",
                  file=sys.stderr)
            continue

        pid = src.get("prompt_id", "")
        prompt = prompt_index.get(pid)
        if prompt is None:
            print(f"[{i}/{n}] {src_path.name}  WARN: prompt {pid!r} not found, skipping",
                  file=sys.stderr)
            continue

        orig_overall = src.get("overall", "?")
        result = replay_one(src, prompt, runner, config, src_path.name)
        new_overall = result["overall"]
        transitions[f"{orig_overall}->{new_overall}"] += 1

        out_path = to_dir / src_path.name
        out_path.write_text(json.dumps(result, indent=2))
        marker = " *** CHANGED" if orig_overall != new_overall else ""
        print(f"[{i}/{n}] {src_path.stem}  {orig_overall} -> {new_overall}{marker}",
              file=sys.stderr)

    print("\n--- transition summary ---", file=sys.stderr)
    for key, count in sorted(transitions.items()):
        print(f"  {key}: {count}", file=sys.stderr)
    print(f"output -> {to_dir}", file=sys.stderr)
    return 0


if __name__ == "__main__":
    sys.exit(main())
