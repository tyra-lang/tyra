#!/usr/bin/env python3
"""bench/ai-gen/harness.py — AI code generation benchmark runner.

Per docs/strategy.md §6.2 item 2: ~100 prompts, 5 languages, measure
compile + execute success rates across frontier models.

Usage:
    python3 harness.py [--languages tyra,ruby] [--generators claude,codex]
                       [--prompts 'prompts/*.yaml'] [--seed 1] [--dry-run]
"""
from __future__ import annotations

import argparse
import glob
import json
import os
import sys
import time
from dataclasses import asdict
from pathlib import Path
from typing import Any, Dict, List

import yaml

HERE = Path(__file__).resolve().parent
REPO_ROOT = HERE.parent.parent
sys.path.insert(0, str(HERE))

from generators import ClaudeGenerator, CodexGenerator  # noqa: E402
from runners import ALL_RUNNERS  # noqa: E402


def load_config() -> Dict[str, Any]:
    with open(HERE / "config.yaml") as f:
        return yaml.safe_load(f)


def load_prompts(patterns: List[str]) -> List[Dict[str, Any]]:
    paths: List[Path] = []
    for pattern in patterns:
        # Relative patterns are resolved from HERE.
        if not os.path.isabs(pattern):
            pattern = str(HERE / pattern)
        for match in sorted(glob.glob(pattern)):
            paths.append(Path(match))
    prompts = []
    for p in paths:
        with open(p) as f:
            data = yaml.safe_load(f)
        if data.get("id") != p.stem:
            print(
                f"warn: {p.name}: id ({data.get('id')!r}) != filename stem ({p.stem!r})",
                file=sys.stderr,
            )
        prompts.append(data)
    return prompts


def make_generators(names: List[str], config: Dict[str, Any]):
    out = []
    if "claude" in names:
        gcfg = config["generators"].get("claude", {})
        out.append(ClaudeGenerator(model=gcfg.get("model")))
    if "codex" in names:
        gcfg = config["generators"].get("codex", {})
        out.append(CodexGenerator(extra_args=gcfg.get("extra_args", [])))
    return out


def decide_overall(
    gen_ok: bool,
    compile_ok: bool | None,
    execute_ok: bool | None,
    checks_passed: bool | None,
    skipped_compiler: bool,
) -> str:
    if skipped_compiler:
        return "skipped_no_compiler"
    if not gen_ok:
        return "generator_fail"
    if compile_ok is False:
        return "compile_fail"
    if execute_ok is False:
        return "exec_fail"
    if checks_passed is False:
        return "check_fail"
    return "pass"


def run_one(
    prompt: Dict[str, Any],
    language: str,
    generator,
    runner_cls,
    config: Dict[str, Any],
    seed: int,
    inject_tyra_spec: bool = False,
) -> Dict[str, Any]:
    runner = runner_cls(config=config, repo_root=REPO_ROOT)
    result: Dict[str, Any] = {
        "prompt_id": prompt["id"],
        "language": language,
        "generator": generator.name,
        "seed": seed,
        "timestamp": int(time.time()),
        "inject_tyra_spec": bool(inject_tyra_spec and language == "tyra"),
    }

    if not runner.compiler_available():
        result["overall"] = "skipped_no_compiler"
        result["stages"] = {}
        return result

    gen = generator.generate(
        prompt["description"], language, inject_tyra_spec=inject_tyra_spec
    )
    result["model"] = gen.model
    result["stages"] = {
        "generate": {
            "ok": gen.ok,
            "duration_ms": gen.duration_ms,
            "code_len": len(gen.code),
            "error": gen.error,
        }
    }
    result["code"] = gen.code

    if not gen.ok:
        result["overall"] = "generator_fail"
        return result

    workdir = runner.prepare_workdir(gen.code)
    try:
        compile_res = runner.compile(workdir)
        result["stages"]["compile"] = {
            "ok": compile_res.ok,
            "duration_ms": compile_res.duration_ms,
            "stderr": compile_res.stderr[-2000:],
            "exit_code": compile_res.exit_code,
            "note": compile_res.note,
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
        must_contain = prompt.get("execution", {}).get("stdout_must_contain", [])
        must_not_contain = prompt.get("execution", {}).get(
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
    ap = argparse.ArgumentParser()
    ap.add_argument("--languages", default="tyra,crystal,v,gleam,ruby")
    ap.add_argument("--generators", default="claude,codex")
    ap.add_argument("--prompts", default="prompts/*.yaml")
    ap.add_argument("--seed", type=int, default=None,
                    help="Single seed (legacy). Prefer --seeds.")
    ap.add_argument("--seeds", default=None,
                    help="Seed list: '1,2,3' or 'N' (=1..N). "
                         "Each seed is a separate run per (prompt, lang, gen).")
    ap.add_argument("--dry-run", action="store_true")
    ap.add_argument(
        "--inject-tyra-spec",
        action="store_true",
        help="Append the Tyra spec + examples to the system prompt when the "
        "target language is Tyra. Runs are written with a '+spec' suffix so "
        "they do not clobber baseline results.",
    )
    ap.add_argument(
        "--results-dir",
        default=str(HERE / "results"),
        help="Directory to write per-run JSON into",
    )
    args = ap.parse_args()

    config = load_config()
    prompts = load_prompts([args.prompts])
    languages = [l.strip() for l in args.languages.split(",") if l.strip()]
    generators_names = [g.strip() for g in args.generators.split(",") if g.strip()]
    seeds = parse_seeds(args.seeds, args.seed)

    missing_langs = [l for l in languages if l not in ALL_RUNNERS]
    if missing_langs:
        print(f"unknown language(s): {missing_langs}", file=sys.stderr)
        return 2

    generators = make_generators(generators_names, config)
    if not generators:
        print(f"no generators selected from {generators_names}", file=sys.stderr)
        return 2

    print(
        f"plan: {len(prompts)} prompts × {len(languages)} languages × "
        f"{len(generators)} generators × {len(seeds)} seeds "
        f"(seeds={seeds}) "
        f"= {len(prompts)*len(languages)*len(generators)*len(seeds)} runs",
        file=sys.stderr,
    )
    if args.dry_run:
        for p in prompts:
            print(f"  prompt {p['id']}: {p['title']}", file=sys.stderr)
        for l in languages:
            runner = ALL_RUNNERS[l](config=config, repo_root=REPO_ROOT)
            status = "ok" if runner.compiler_available() else "MISSING"
            print(f"  language {l}: {status}", file=sys.stderr)
        return 0

    results_dir = Path(args.results_dir)
    results_dir.mkdir(parents=True, exist_ok=True)

    completed = 0
    for prompt in prompts:
        for language in languages:
            runner_cls = ALL_RUNNERS[language]
            for generator in generators:
                for seed in seeds:
                    completed += 1
                    # Suffix spec-injected Tyra runs so they sit side by side
                    # with the zero-corpus baseline rather than overwriting it.
                    spec_suffix = (
                        "+spec"
                        if args.inject_tyra_spec and language == "tyra"
                        else ""
                    )
                    key = (
                        f"{prompt['id']}__{language}{spec_suffix}"
                        f"__{generator.name}__s{seed}"
                    )
                    out_path = results_dir / f"{key}.json"
                    print(f"[{completed}] {key}", file=sys.stderr)
                    if out_path.exists() and not getattr(args, "overwrite", False):
                        print(f"    -> skipped (exists)", file=sys.stderr)
                        continue
                    try:
                        result = run_one(
                            prompt=prompt,
                            language=language,
                            generator=generator,
                            runner_cls=runner_cls,
                            config=config,
                            seed=seed,
                            inject_tyra_spec=args.inject_tyra_spec,
                        )
                    except Exception as e:
                        result = {
                            "prompt_id": prompt["id"],
                            "language": language,
                            "generator": generator.name,
                            "seed": seed,
                            "overall": "harness_error",
                            "error": f"{type(e).__name__}: {e}",
                        }
                    with open(out_path, "w") as f:
                        json.dump(result, f, indent=2)
                    print(f"    -> {result.get('overall')}", file=sys.stderr)
    return 0


def parse_seeds(seeds_arg: str | None, seed_arg: int | None) -> List[int]:
    """Resolve --seeds / --seed into an explicit seed list.

    Precedence: --seeds wins if given; otherwise fall back to --seed;
    otherwise default to [1] so existing invocations keep working.
    """
    if seeds_arg:
        s = seeds_arg.strip()
        if "," in s:
            return [int(x) for x in s.split(",") if x.strip()]
        n = int(s)
        if n <= 0:
            raise ValueError(f"--seeds must be positive, got {n}")
        return list(range(1, n + 1))
    if seed_arg is not None:
        return [seed_arg]
    return [1]


if __name__ == "__main__":
    sys.exit(main())
