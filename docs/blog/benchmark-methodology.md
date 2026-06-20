# Measuring first-try LLM correctness: how we got 88.7% (3 seeds × 100 prompts), and how to rerun it

Tyra is a young language with almost no public corpus. That is normally a problem when you want a large language model to write it. A model has never seen Tyra on GitHub, in a tutorial, or in a Stack Overflow answer, so on its own it produces nothing that compiles. We wanted to know something narrower and more useful: if you hand the model the language definition and nothing else, how often does it write code that compiles and runs correctly on the first try?

This article explains exactly how we measured that, what the number means, and — at equal length — what it does not mean. The headline is 88.7% mean pass across 3 seeds × 100 prompts on Tyra v0.11.0. Everything below is here so you can check that, reproduce it, and disagree with it.

## What "first-try correctness" means

First-try correctness is a single round: the model is given a task, it emits one program, and that program is graded with no human in the loop. No iteration. No "fix the error and try again." No human selecting the best of several outputs. The model writes once, and we grade what it wrote.

This is worth measuring for a young language specifically because there is no corpus to lean on. A language that a model can write correctly from its definition alone is a language whose definition is unambiguous enough to be learned from that definition. That is the property we care about, and it is the same property a human reader benefits from. The benchmark is a proof point for that thesis, not the thesis itself.

## The method

The task set is 100 plain-English programming tasks, one YAML file per task. The descriptions are deliberately neutral: no task mentions a language, a framework, a standard-library function, or a syntax hint. The same 100 tasks are used for every language we test. They live in the repository under `bench/ai-gen/prompts/` and are open to inspection.

For each task, the model receives a system prompt and the task description, and returns one program. For Tyra, the system prompt has the full English language specification, all the canonical example programs, and the entire standard-library source appended to it. We call this spec injection, and it is central to the result:

**Without spec injection, Tyra scores 0 out of 100.** The model does not know the syntax and falls back to Rust-, Gleam-, or Scala-flavored guesses, none of which compile. The 88.7% is the score *with* the spec in the prompt. This is intentional and it is the whole point — the question is whether the definition is enough — but it has to be stated plainly, because it is the first thing a fair reader needs to know.

The base system prompt is otherwise identical across every language in the suite. Only the injected reference material differs, so what varies between languages is the model's prior familiarity, not the scaffolding around it.

### Grading

Each generated program goes through three stages, and the harness records a specific outcome for each run:

1. **Generate** — the model must return a non-empty source file.
2. **Compile** — the Tyra compiler must exit 0 (for Ruby, this stage is `ruby -c`; more on that below).
3. **Execute** — the compiled binary must exit 0.

After execution, the harness applies a marker check: every required string in `stdout_must_contain` must appear in the program's output, and no string in `stdout_must_not_contain` may appear.

That produces four possible outcomes, and it is worth naming them exactly as the harness does, because the difference matters:

- **pass** — compiled, ran, exited 0, and the output markers matched.
- **compile_fail** — the compiler rejected the program.
- **exec_fail** — the binary compiled but exited non-zero.
- **check_fail** — the binary compiled and exited 0, but a required output marker was missing or a forbidden one appeared. A program that compiles and runs cleanly but computes the wrong answer lands here, not in `exec_fail`.

There is no partial credit. A run is a pass only if it clears every stage.

The marker check is deliberately weaker than exact stdout equality and stronger than "it compiled." Exact-output matching would punish trivial formatting differences across six languages; "it compiled" would reward programs that do nothing. Markers sit in between, and they apply uniformly to every language. This also means the benchmark does not verify full functional correctness: a program whose logic is wrong but whose output happens to contain the required markers will pass. We return to that limit below.

## The numbers

The headline run is `run56`, against the Tyra v0.11.0 compiler, with three seeds over the 100-task set — 300 runs in total. The full breakdown:

- **Mean pass: 88.7%** — 266 of 300 runs passed.
- **Any-seed: 98%** — 98 of 100 prompts passed on at least one of the three seeds.
- **All-seed: 77%** — 77 of 100 prompts passed on all three seeds.

These are three different measurements and it is worth keeping them apart. The mean (88.7%) is the per-run pass rate across all 300 runs. Any-seed (98%) asks how many tasks the model can do at least sometimes. All-seed (77%) asks how many it does reliably every time. "First try" maps to the mean: 266 of 300 single-shot attempts produced correct programs.

The 34 non-passing runs break down as:

- **29 compile_fail** (9.7% of 300)
- **3 exec_fail**
- **2 check_fail**

That accounts for all 300 runs: 266 + 29 + 3 + 2.

Only two prompts failed on all three seeds: `034-group-even-odd` and `096-rate-limit`. Those are the two tasks the current compiler and the model, together, cannot yet do reliably from the spec alone, and they are named here so the failure surface is concrete rather than implied.

## What this does *not* compare

You will find a five-language table in the repository. It looks like this:

| Language | Pass |
|---|---|
| Ruby | 99% |
| Crystal | 96% |
| Go | 81% |
| V | 49% |
| Gleam | 37% |

It is tempting to read that as a ranking. It is not one, and here is why in plain terms.

Those figures are **single-seed point estimates drawn from a separate, earlier sweep against a different (and now stale) Tyra binary**. They were not produced under the same conditions as `run56`. Go, for example, was measured on seed 1 only, in its first and only sweep. The other rows draw on accumulated results from prior runs at different seed counts. The repository itself describes this table as a mixed historical aggregate that is not directly comparable, and we mean it: it is directional context, not a controlled comparison.

We do not claim Tyra beats Go, or any other language in that table. A single-seed number against a different binary is not a result you can rank against a three-seed mean. The honest statement is the one this article is built around: we have a multi-seed number for Tyra, and we do not yet have one for the others.

Two specific asterisks matter even for directional reading:

- **Ruby's 99% reflects training data, not language design.** Ruby is one of the most-represented languages in any model's training set; the model writes it fluently from memory. The benchmark's five-language ordering tracks corpus volume more than anything about the languages. Note too that Ruby's "compile" stage is only `ruby -c`, a syntax check rather than a type check — a floor, not a ceiling.
- **Gleam's 37% is depressed by harness overhead.** The Gleam runner wraps each program in a template project, and that wrapping accounts for some of the failures. The number understates Gleam's real capability.

### What would make this a fair comparison

A fair comparison is a single controlled sweep: the same generator, the same seed set, all six languages, the same current binaries, run together. **That sweep has not been run.** It is the obvious next step, it is listed as an open follow-up in the repository, and until it exists, the cross-language numbers stay in the "directional only" box. We would rather say that than imply a ranking the data does not support.

## A known traceability gap

One more thing a careful reader will catch if they open the result files, so we will say it first. The `run56` result JSONs record the model as `2.1.174 (Claude Code)` — a CLI version string, not a pinned model identifier. The benchmark config ships with `model: null`, which lets the CLI use its configured default. So the exact frontier model behind the headline run is not pinned in the artifacts.

This is a real limitation in traceability, and it is worth contrasting with an earlier run that did pin its model: Run 53 (2026-05-15) fixed `claude-sonnet-4-6` in its config and is a separate, higher-scoring run — do not conflate the two. For `run56`, the honest statement is that the model was a recent Claude accessed through the Claude Code CLI at version 2.1.174, and that we did not pin a model ID in the config. Future runs should.

## How to reproduce it

The harness is in `bench/ai-gen/`. The Tyra compiler is built from the repository (`cargo build -p tyra-cli`) or pointed at via the `TYRA_BIN` environment variable; to reproduce `run56` specifically, build the v0.11.0 compiler (`git checkout v0.11.0`). One honest caveat on traceability: each result JSON records the *generator's* CLI version (in the `model` field), but it does **not** embed the Tyra compiler version — so a result is tied to its compiler by the run label, not by the file itself. Pinning the compiler version into every result is an obvious improvement, and it is not done yet.

From `bench/ai-gen/`, the Tyra-with-spec sweep is:

```
python3 harness.py --inject-tyra-spec --languages tyra --generators claude --seeds 3
```

The `--seeds 3` argument resolves to seeds 1..3 (the flag accepts either an explicit list like `1,2,3` or `N` meaning `1..N`), which reproduces the three-seed design of `run56`. Drop `--inject-tyra-spec` and you will see the zero-corpus baseline: the model no longer knows the language, and the pass rate collapses to 0. The prompts are versioned in git, so any change to a task description is a breaking change that requires a fresh full sweep; the set you run is the set in your checkout.

Full details, including per-language runner specifics and the exact stage definitions, are in `bench/ai-gen/METHODOLOGY.md` and `bench/ai-gen/INSIGHTS.md`.

## What this proves, and what it does not

It proves a narrow, real thing: given the Tyra specification and nothing else, a current frontier model writes a program that compiles, runs, and produces the expected output markers on 266 of 300 single-shot attempts — 88.7% — with 98% of tasks passing on at least one seed and 77% passing on all three. For a language with no public training data, the definition is doing the work. That is the claim, and it is the property the language was designed for.

It does not prove the following, and we will not stretch it to:

- It does not prove **functional correctness**. The marker check can pass on a program whose computation is wrong but whose output happens to match.
- It does not prove anything about **code quality, performance, or maintainability**. None of those are measured.
- It does not establish a **human baseline**. We do not know how an expert Tyra programmer scores on the same tasks under the same one-shot rule.
- It does not show that Tyra is **easier or better than any other language**. The cross-language table is directional only; the controlled sweep that could support a comparison has not been run.

What is left after all those subtractions is still worth stating plainly: a young language with no corpus can be written correctly, first try, from its definition alone, most of the time. It is a proof point for interpretive consistency, not a victory lap. The benchmark is open, the prompts are open, the harness is open, and the one comparison people will most want — the controlled cross-language sweep — is the one we are telling you we have not done yet. Run it and tell us what you find.
