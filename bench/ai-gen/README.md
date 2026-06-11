# ai-gen — AI Code Generation Benchmark

Per `docs/strategy.md` §6.2 and §4.1, Tyra's strongest potential
differentiator is measurable AI auditability. This harness quantifies
that claim by asking frontier models to generate the same programs in
Tyra, Crystal, V, Gleam, Ruby, and Go, then grading each output
through three stages: **generation**, **compile / type-check**, and
**execution**.

See [METHODOLOGY.md](METHODOLOGY.md) for prompt neutrality policy,
scoring criteria, model pinning strategy, and threats to validity.

## Headline metrics (per language × model pair)

| Metric | Meaning |
|---|---|
| `pass` | Generated, compiled, executed, and matched all expected output markers |
| `compile_fail` | Compiler / type-checker rejected the output |
| `exec_fail` | Compiled but crashed, timed out, or produced wrong output |
| `generator_fail` | Model refused or returned unparseable output |
| `skipped_no_compiler` | Compiler not found on host; run skipped (not an error) |

## Layout

```
bench/ai-gen/
  README.md           this file
  METHODOLOGY.md      prompt neutrality, scoring, model pinning, validity
  harness.py          main entrypoint — orchestrates prompt × lang × model
  report.py           aggregates results/*.json into Markdown + CSV
  config.yaml         languages, models, timeouts, compiler commands
  requirements.txt    Python deps (anthropic, pyyaml)
  prompts/            one YAML per prompt (schema below)
  generators/
    base.py           abstract Generator
    claude.py         `claude -p` CLI subprocess wrapper
    codex.py          `codex` CLI subprocess wrapper
  runners/
    base.py           abstract Runner (compile + execute)
    tyra.py           tyra binary (TYRA_BIN env or in-repo build)
    crystal.py        `crystal build`
    v.py              `v -o` build then exec
    gleam.py          injects source into templates/gleam/ project
    ruby.py           `ruby -c` syntax check, then `ruby` to exec
    go.py             `go build` with isolated GOCACHE, then exec
  templates/gleam/    minimal Gleam project template
  results/            committed summary + JSON artifacts (run dirs gitignored)
```

## Prerequisites

Install language compilers:

| Language | Install |
|---|---|
| Tyra | `cargo build -p tyra-cli` (in-repo), or set `TYRA_BIN=/path/to/tyra` |
| Go | [go.dev/dl](https://go.dev/dl) |
| Crystal | `brew install crystal` (macOS) / apt crystal |
| V | [vlang.io](https://vlang.io) |
| Gleam | `brew install gleam` (macOS) / [gleam.run](https://gleam.run) |
| Ruby | System ruby or rbenv |

Install Python dependencies:

```bash
cd bench/ai-gen
pip install -r requirements.txt
```

Authenticate AI generators:

```bash
# claude CLI — one-time login
claude /login

# codex CLI — one-time auth
codex auth
```

## Quick start

```bash
cd bench/ai-gen

# Dry run — validate prompts, print the execution plan
python3 harness.py --dry-run

# Run all 100 prompts, all 6 languages, claude generator only, seed 1
python3 harness.py --generators claude

# Run a single language
python3 harness.py --languages tyra --generators claude

# Run with an installed tyra binary instead of in-repo build
TYRA_BIN=~/.local/bin/tyra python3 harness.py --languages tyra --generators claude

# Aggregate results into Markdown
python3 report.py > results/SUMMARY.md
```

Flags:

| Flag | Default | Meaning |
|---|---|---|
| `--languages` | all | Comma-list of languages to run |
| `--generators` | all | `claude`, `codex`, or both |
| `--prompts` | `prompts/*.yaml` | Glob for prompt files |
| `--seed` / `--seeds` | 1 | Seed or seed range (`1,2,3` or `N`=1..N) |
| `--dry-run` | off | Print plan without running |
| `--inject-tyra-spec` | off | Append full spec to Tyra system prompt |
| `--results-dir` | auto | Override the per-run output directory |

## Prompt schema

```yaml
id: "001-fizzbuzz"              # must match filename stem
title: "FizzBuzz"
description: |                  # prose handed verbatim to the model
  Read an integer N from stdin. For i in 1..=N, print i on its own
  line, but print "Fizz" for multiples of 3, "Buzz" for multiples
  of 5, and "FizzBuzz" for multiples of both.
tags: [loops, stdin]
execution:
  stdin: "15\n"
  stdout_must_contain: ["Fizz", "Buzz", "FizzBuzz", "14"]
  stdout_must_not_contain: []
  timeout_seconds: 10
```

Every language receives the identical prose description with no
language-specific syntax hints. See [METHODOLOGY.md](METHODOLOGY.md)
for the full neutrality policy.

## Evaluation pipeline

1. **generate** — model returns a code string. Failure = API error,
   refusal, or empty output.
2. **compile** — runner compiles (or `ruby -c` for Ruby). Type errors
   → `compile_fail`.
3. **execute** — runner feeds `execution.stdin`, waits up to
   `timeout_seconds`, captures stdout/stderr and exit code. `pass`
   requires exit 0 + all `stdout_must_contain` markers present + no
   `stdout_must_not_contain` string present.

An earlier stage failing short-circuits the rest.

## Reproducing published results

Results committed to `results/SUMMARY.md` were produced from a
specific tyra version and model config. To reproduce:

```bash
# 1. Build the same tyra version
git checkout <tag>
cargo build --release -p tyra-cli

# 2. Run the sweep (or use TYRA_BIN for an installed binary)
python3 bench/ai-gen/harness.py \
  --generators claude \
  --seeds 3

# 3. Aggregate
python3 bench/ai-gen/report.py > bench/ai-gen/results/SUMMARY.md
```

The per-run directories (`results-run*/`) are gitignored. Only the
aggregated `results/SUMMARY.md` is committed.

## Repo / site split

- **This repo** — benchmark code, prompts, raw methodology, and the
  aggregated `results/SUMMARY.md`.
- **tyra-lang/site** (separate repo, future) — the public-facing
  comparison page that displays these numbers.

## Caveats

- **Non-determinism** — frontier models are sampled; every run
  produces different code. The harness stores the raw generated code
  alongside the pass/fail verdict so reruns can be diffed.
  Aggregate over ≥3 seeds for stable headline numbers.
- **Codex CLI** — uses whatever model the local install is configured
  for; the harness records that model name in each result JSON but
  cannot force a specific model version.
- **Gleam** — the runner wraps generated code in
  `templates/gleam/src/aigen_bench.gleam`; single-file generation
  does not match Gleam's native project structure, so results may be
  pessimistic.
- **Costs** — 100 prompts × 6 languages × N generators × N seeds adds
  up quickly. Default `config.yaml` runs seed=1; bump `--seeds`
  deliberately.
