# ai-gen — findings

Running log of observations that raw counts in `SUMMARY.md` hide.

## Run 3 (2026-04-21) — 40 prompts × 5 languages × {codex, claude}

### Headline

| language | claude pass% | codex pass% (adj) | codex raw% |
| -------- | -----------: | ----------------: | ---------: |
| ruby     | 100.0 (39/39) | 95.2 (20/21) | 40.8 |
| crystal  |  92.3 (36/39) | 83.3 (10/12) | 25.0 |
| v        |  56.4 (22/39) | 66.7 (8/12)  | 20.0 |
| gleam    |  38.5 (15/39) | 54.5 (6/11)  | 15.0 |
| tyra     |   0.0  (0/39) | 16.7 (3/18)  |  6.1 |

Codex "adjusted" excludes `generator_fail` (CLI timeouts @ 180s).
The Claude column never times out — the `claude -p` pathway is
dramatically more stable and faster (~6–10 s per call vs. Codex's
~40 s when it doesn't time out).

### Ranking is stable across generators

Both models rank the languages the same way:
**Ruby > Crystal > V > Gleam > Tyra.** This ordering correlates
monotonically with each language's presence in public code corpora
— Ruby is decades of Rails, Crystal adds a ~10-year Ruby-flavored
corpus, V and Gleam are newer and smaller, Tyra has literally zero
pre-existing public code. Claude and Codex both agree on the
ordering, which is what you expect when training data volume, not
language design, is the dominant variable.

### Claude Tyra is 0 / 39. Why

Every Claude Tyra attempt compile_failed. Sampling the generated
source reveals that Claude does not know Tyra's syntax at all:

- `001-fizzbuzz`: emitted Rust (`use std::io;`, `fn main() { ... }`,
  `for i in 1..=n { ... }`)
- `002-word-count`: emitted Gleam-flavored syntax
  (`import std/io`, `import std/strings`)
- `003-option-chain`: emitted a Scala/Haskell hybrid (`Option[Int]`,
  `match foo { Some(v) => ... }`)
- `004-state-machine`: used `{}` brace blocks where Tyra requires
  `end` keywords; the compiler actually got as far as a clean
  syntax error (`expected 'end', found end of file`)

Codex's 16.7% is not that Codex knows Tyra better — it's that Codex
spends longer per turn, occasionally stumbling into `end`-block
syntax that happens to compile. Neither model has real knowledge.

This is the zero-corpus baseline. The benchmark will become
meaningful once Tyra has a published corpus (docs, examples,
tutorials, GitHub presence). Until then the test mostly measures
"did Claude's fallback guess land on Tyra-compatible syntax."

### Compiler bug uncovered

In a majority of Tyra compile_fails, the compiler panics with

    thread 'main' panicked at
    compiler/crates/tyra-parser/src/token_stream.rs:218:22:
    index out of bounds: the len is N but the index is N

rather than emitting a diagnostic. The input files are
garbled-by-the-model Rust/Gleam/etc., but the parser should reject
them cleanly, not crash. Candidate fix: bounds-check the token
pointer advance in token_stream.rs:218 and return a synthetic
diagnostic on EOF overrun. Worth a separate small ticket.

### Stdlib-gap observations (from codex run 2)

Even Codex's best Tyra attempts tripped over stdlib gaps:

- `import fs` for stdin: Tyra v0.1 has file-only fs (§17.3.1); no
  stdin module ships. Many codex attempts started `import fs;
  let input = fs.read_stdin()` and died at resolve time.
- No `%` operator: the `value / d * d == value` workaround codex
  tried parsed incorrectly in `else if` chains.

These are actionable: an `io` module with `read_line` /
`read_to_end`, plus a `%` or `rem` fn, would clear the
largest chunk of Tyra's compile_fails. Doing this is the first
roadmap-level intervention to make the benchmark fair.

## What this tells us about strategy.md §4.1

The headline claim — "Tyra's design makes AI-generated code
measurably more compile-reliable than competitors" — **cannot** be
validated or falsified in the zero-corpus regime the benchmark
currently measures. The 5-language ordering visible here is a
corpus-size ordering, not a design-quality ordering. To defend
strategy.md §4.1 we need:

1. **Tyra has to exist in training data.** Publish spec, stdlib,
   examples, and enough tutorial-quality code on public hosts
   (GitHub) that a model's next training run ingests them. This is
   months-to-year work.
2. **Rerun after one frontier-model refresh cycle.** If Tyra's
   compile rate catches up to (or exceeds) Crystal at similar
   corpus size, the design claim has support.
3. **Add a Tyra-aware variant.** As a control, feed the model the
   spec + example programs in-prompt (RAG / long-context) and
   rerun. If the compile rate still underperforms Crystal given
   the same spec exposure, the design thesis is in trouble.

In the short term, **the cheapest wins for Tyra's bench numbers
are not model-side — they are stdlib-side.** An `io` module and a
modulo operator / function address the bulk of concrete compile
failures.

## Methodology notes carried forward

- Single seed per (prompt, lang, gen). Frontier models sample;
  publishable headlines should average ≥ 3 seeds.
- Codex rate-limits persistently if swept too long; budget
  cooldowns between sweeps.
- Claude cwd is pinned to `/tmp` so project CLAUDE.md does not leak
  into context.
- `ruby -c` is a syntax check, not a type check — the "compile"
  stage for Ruby is a floor, not a ceiling. Ruby's 100% is a
  weaker claim than Crystal's 92%.
- Prompts do not hint stdlib or idioms. The point is to measure
  what the model writes by default. If a language wants to improve
  its number, the fix is usually documentation (so the model's
  next training ingests better examples), not prompting.

## Next experiments

1. **Tyra stdlib `io` + modulo.** Spec + implement. Then re-run
   prompts 001-040 on Tyra only.
2. **Prompts 041-100 on all 5 × claude.** The Claude pass is fast
   and stable; finish the full 100-prompt set. ~100 × 5 × 7s ≈ 1 h.
3. **Parser bounds fix** at `tyra-parser/src/token_stream.rs:218`.
   Isolate one crashing input, add a regression test, patch.
4. **Multi-seed sweep** (seed=1,2,3) for the top-tier languages to
   estimate variance.

## Run history

- **Run 1** (2026-04-21) — 9 prompts × {tyra, ruby} × codex.
  Baseline smoke. Superseded by Run 2.
- **Run 2** (2026-04-21) — 30 prompts × 5 langs × codex. First
  cross-language data. Tail hit codex rate limit.
- **Run 3** (2026-04-21) — 40 prompts × 5 langs × claude. Clean,
  no timeouts. Revealed zero-corpus Tyra + token_stream parser
  panic.
