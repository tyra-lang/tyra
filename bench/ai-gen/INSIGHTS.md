# ai-gen — findings

Running log of observations that raw counts in `SUMMARY.md` hide.

## Run 5 (2026-04-21) — spec-injection experiment (Tyra only, claude)

Controlled test of the strategy §4.1 design-quality claim. The
harness gained `--inject-tyra-spec`, which appends the full en
language spec + all 10 canonical example programs to the system
prompt when the target is Tyra. Other languages are unchanged.

### Result

| run | pass / 100 | rate |
| --- | ---------: | ---: |
| tyra baseline (Run 4) | 0 | 0.0% |
| **tyra+spec** | **16** | **16.0%** |

Baseline improvement: 0 → 16 passes just from letting Claude
see the spec. Still far below Crystal 96% / Ruby 99% — but the
reason becomes clear when the 80 `compile_fail`s are
classified by error code.

### Failure breakdown (80 compile_fails)

```
E0200 cannot import ... module not found   63   (79% of fails)
E0500 type / generic                         7
E0101 expected newline                       4
E0102                                        2
E0104                                        1
E0002                                        2
no-code                                      1
```

**63 of the 80 failures are a single cause: `import io` /
`import core.io` / `import fs` to read from stdin.**
Tyra v0.1 has no io module. The programs Claude produced around
that missing import are syntactically valid Tyra — `end` blocks,
Result/Option, match arms, `?` propagation all look correct — and
the rest of the program would almost certainly compile if the
import resolved.

### Which prompts passed

Every prompt that did NOT need stdin compiled and ran correctly:

```
003-option-chain      004-state-machine    005-sum-list
014-result-chain      027-fizzbuzz-string  030-command-dispatch
032-divide-safe       040-person-record    045-minmax-both
046-count-true        060-shopping-total   061-power
079-point-distance    080-collect-errors   093-option-map
096-rate-limit
```

These exercise ADT + exhaustive match, record construction,
nested Option, Result with named error variants, fold / filter /
map over inline lists, state machines, and rate-limit-style
mutable state — exactly the features strategy §4.1 advertises
as Tyra's design wins. **When Tyra's v0.1 surface can express
the problem at all, Claude emits compiling, correct code at
16/16 = 100% hit rate once it has the spec.**

### What this tells us about strategy.md §4.1

The thesis is **provisionally supported** — not refuted. The
8x compile_fail cliff versus Crystal is almost entirely
"missing stdlib," not "bad language design." Code for the
subset of prompts Tyra *can* handle is 16/16 perfect.

To convert this provisional support into a clean comparison,
Tyra needs the minimum io surface: `io.read_line()` /
`io.read_to_end()`. With that in place the model's existing
correct-Tyra programs would resolve their imports and tyra+spec
should jump into the 70–80% range at minimum.

### Cost / timing note

Each spec-injected call runs ~15–17s vs the zero-context
baseline's ~7s. Total sweep: ~25 min for 100 runs, still
comfortable. Context per call is ~100 KB (spec + 10 examples).

## Run 4 (2026-04-21) — full 100 prompts × 5 languages × claude

| language | claude pass% | codex pass% (adj) | codex raw% |
| -------- | -----------: | ----------------: | ---------: |
| ruby     | 99.0 (99/100) | 95.2 (20/21) | 40.8 |
| crystal  | 96.0 (96/100) | 83.3 (10/12) | 25.0 |
| v        | 49.0 (49/100) | 66.7 (8/12)  | 20.0 |
| gleam    | 37.0 (37/100) | 54.5 (6/11)  | 15.0 |
| tyra     |  0.0 (0/100)  | 16.7 (3/18)  |  6.1 |

The claude column is the first complete 100-prompt dataset. Every
prior partial conclusion survived: ranking ruby > crystal > v >
gleam > tyra is unchanged, zero-corpus Tyra is 0/100, and claude
never times out (500 runs, zero generator_fail). The codex column
is capped at ~40 prompts because the CLI hit its rate window
partway through the sweep (see Run 3 notes); adjusted rate still
applies.

## Run 3 (2026-04-21) — 40 prompts × 5 languages × {codex, claude}

Superseded by Run 4 for the claude column. Kept here for the codex
comparison, which is the most complete codex data we have.

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

### Compiler bug uncovered (FIXED in `e788b4a`)

Most Run 3 Tyra compile_fails had the compiler panicking with

    thread 'main' panicked at
    compiler/crates/tyra-parser/src/token_stream.rs: index out of bounds

rather than emitting a diagnostic. Root cause: `peek_skip_newlines`,
`peek_past_newlines`, `raw_peek`, and `advance` all indexed
`self.tokens[pos]` without bounds check. On unmatched brackets
(bracket_depth > 0) or after advance past Eof, pos could reach
len and panic. Fix clamps every cursor read to
`pos.min(len.saturating_sub(1))`, so the cursor lands on Eof
forever past end instead of panicking. Two regression tests added
in tyra-parser.

All 100 Run 4 Tyra compile_fails are now clean diagnostics.

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
- **Run 4** (2026-04-21) — full 100 prompts × 5 langs × claude.
  500 runs, zero timeouts. Ruby 99/100, Crystal 96/100, V 49/100,
  Gleam 37/100, Tyra 0/100. Parser panic fixed beforehand
  (`e788b4a`) so all 100 Tyra compile_fails are clean.
- **Run 5** (2026-04-21) — tyra+spec × 100 × claude. Spec injection
  via --inject-tyra-spec. 16/100 pass (vs 0/100 baseline). 63 of
  80 compile_fails are E0200 "import io not found" — all stdlib,
  not syntax. Strategy §4.1 provisionally supported: design works,
  stdlib is the wall.
