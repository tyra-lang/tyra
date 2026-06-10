# ai-gen — AI Code Generation Benchmark

Per `docs/strategy.md` §6.2 item 2 and §4.1, Tyra's strongest potential
differentiator is measurable AI auditability. This harness quantifies
that claim by asking Claude and the Codex CLI to generate the same
programs in Tyra, Crystal, V, Gleam, Ruby, and Go, then grading each
output through three stages: **generation**, **compile / type-check**,
and **execution**.

The methodology mirrors the strategy doc's target: ~100 prompts, 6
languages, ≥1 frontier model. Headline metric per (language, model)
pair:

- `pass` rate — code compiled and executed and produced the expected
  stdout markers
- `compile_fail` rate — compiler / type-checker rejected the output
- `exec_fail` rate — compiled but crashed, hung, or produced wrong
  output
- `generator_fail` — model refused or produced unparseable output
- `skipped_no_compiler` — the compiler is not installed on the host
  (the harness never fails the whole run on a missing toolchain)

## Layout

```
bench/ai-gen/
  README.md           this file
  harness.py          main entrypoint; orchestrates prompt × lang × model
  report.py           aggregates results/*.json into Markdown + CSV
  config.yaml         languages, models, timeouts, compiler commands
  requirements.txt    Python deps (anthropic, pyyaml)
  prompts/            one YAML per prompt (schema below)
  generators/
    base.py           abstract Generator
    claude.py         `claude -p` CLI subprocess wrapper
    codex.py          `codex exec` subprocess wrapper
  runners/
    base.py           abstract Runner (compile + execute)
    tyra.py           shells to the in-repo `tyra` binary
    crystal.py        `crystal build`
    v.py              `v run`
    gleam.py          injects source into templates/gleam/
    ruby.py           `ruby -c` for type/syntax, then `ruby` to exec
    go.py             `go build` with isolated GOCACHE, then exec
  templates/gleam/    minimal Gleam project template
  results/            JSON artifacts, one per (prompt, lang, gen) triple
```

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

Every language is given the same prose description. Prompts intentionally
avoid language-specific type or syntax hints; the whole point is to
measure how well each language's idioms align with what a frontier model
spontaneously produces.

## Running

```sh
cd bench/ai-gen
pip install -r requirements.txt
# Both generators authenticate via the installed CLI's own config.
# - claude: `claude /login` first if not already logged in
# - codex:  `codex auth` first if not already logged in
python3 harness.py --languages tyra,ruby --generators claude
python3 report.py > results/SUMMARY.md
```

Flags:

- `--languages` — comma-list (default: all). Missing compilers are
  skipped, not errored.
- `--generators` — `claude`, `codex`, or both (default: both).
- `--prompts` — glob (default: `prompts/*.yaml`).
- `--dry-run` — load + validate prompts and print the plan.

## Evaluation stages

1. **generate** — generator returns a code string. Failure = API error,
   refusal, or empty output.
2. **compile** — language runner compiles (or, for Ruby, runs
   `ruby -c`). Type errors count as `compile_fail`.
3. **execute** — runner feeds `execution.stdin` to the binary, waits
   up to `timeout_seconds`, captures stdout/stderr and exit code.
   `pass` requires exit 0 + all `stdout_must_contain` strings present
   + no `stdout_must_not_contain` string present.

Any earlier stage failing short-circuits the rest.

## Why compile + exec, not functional correctness

The harness does not verify answer correctness by equality — that would
require 100 reference implementations × 6 languages. Instead, each
prompt declares a few **markers** the output must include. This is a
weaker signal than full correctness but a strictly stronger signal
than "it compiled," and is uniform across languages with wildly
different stdlibs. See `docs/strategy.md` §4.1 for how this feeds into
the acquisition narrative.

## Caveats

- **Non-determinism** — frontier models are sampled. Every run produces
  a different code string. The harness records raw code alongside the
  pass/fail verdict so reruns can be diffed. Aggregates should average
  over ≥3 runs for headline numbers.
- **Codex CLI** — wraps whatever model the local Codex install is
  configured to use. The harness records that model name in the result
  for traceability; it does not force a specific model.
- **Gleam** — single-file generation does not match Gleam's project
  model. The runner writes the generated code into
  `templates/gleam/src/main.gleam` before invoking `gleam run`.
- **Tyra** — uses the in-repo debug binary at
  `target/debug/tyra`. Run `cargo build -p tyra-cli` first.
- **Costs** — 100 prompts × 6 languages × 2 generators × N seeds gets
  expensive fast. Default `config.yaml` caps at seed=1; bump manually.
