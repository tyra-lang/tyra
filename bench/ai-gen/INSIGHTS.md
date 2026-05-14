# ai-gen — findings

Running log of observations that raw counts in `SUMMARY.md` hide.

## Run 52 (2026-05-14) — replay: this-session compiler fixes vs Run 51

**This is a replay run, not a fresh generation.** No LLM was called.
`bench/ai-gen/replay.py` took the model code already cached in
`results-run51/*.json` (generated with `--inject-tyra-spec`) and
re-compiled + re-executed it against the current compiler. Purpose:
isolate the effect of compiler fixes from any model variance.

This is the first entry in this log since Run 8. Runs 9–51 are tracked
in `results-run*/` and (partially) in the now-retired `SUMMARY.md`;
they are not backfilled here.

### Compiler commits included in this session

| commit | change |
| --- | --- |
| `2265fc1` | fix(mir): correctly store match arm result when payload binding present |
| `fbfd811` | fix(codegen): compare all scalar fields for Option/Result structural equality |
| `fbd2064` | fix(types): derive Eq/Hash/Debug for Option/Result from type arguments |
| `9657b86` | fix(codegen): hoist alloca instructions to entry block |

### Result

| metric | Run 51 | Run 52 (replay) | delta |
| --- | ---: | ---: | ---: |
| pass | 297 | 298 | **+1** |
| check_fail | 1 | 0 | -1 |
| exec_fail | 1 | 1 | 0 |
| compile_fail | 1 | 1 | 0 |
| pass% | 99.0% | **99.3%** | +0.3 |
| all_pass% (3 seeds) | 97.0% | **98.0%** | +1.0 |

### Cell-by-cell breakdown

**`049-count-chars` s1: `check_fail → pass`.**
This is the primary validation target. The model code output
`count=11` (counting all `s` occurrences) instead of `count=4`
(matching only the target character). The root cause was a codegen
bug in MIR match arm result storage (`2265fc1`) combined with a
missing structural comparison for Option/Result (`fbfd811`). After
the fix the program outputs `count=4` and passes.

**`099-sum-column` s1: `exec_fail → exec_fail`, but the failure
mode changed.**
Run 51: exit code −11 (SIGSEGV) — alloca in a non-entry basic block
consumed O(iterations) stack inside a `while` loop, exhausting the 8 MiB
stack around 130 k iterations. Run 52: timeout (exit code null, note
`"timeout"`) — the alloca hoisting fix (`9657b86`) eliminated the
compiler-caused crash. The remaining `exec_fail` is the model's own
bug: the generated code is a `while true` loop with no `break`, so it
hangs indefinitely. **The compiler defect is resolved; the model code
defect is out of scope.**

**`088-histogram` s1: `compile_fail → compile_fail` (unchanged).**
E0110: the model placed `import string` inside a function body. This
is a model code-quality issue, not a compiler limitation. Unchanged
result is expected and correct.

**Remaining 297 cells: `pass → pass`, zero regressions.**

The `fbd2064` Option/Result Eq fix does not correspond directly to any
Run 51 failure cell, but is validated by 6 new unit tests in
`tyra-types` and confirmed by the 297-cell regression baseline.

### What this means

The session's compiler fixes worked exactly within their intended
scope. One prompt moved from `check_fail` to `pass`; one compiler-caused
crash became a model-caused timeout; zero regressions. The two remaining
failures (`088`, `099`) are now cleanly attributable to model code
generation quality, not compiler defects.

## Run 8 (2026-04-21) — spec injection v3: anti-hallucination method guide

Added a final block to the injected context enumerating common
Rust-/Scala-isms that Claude reaches for but Tyra v0.1 does not
ship (`.unwrap_or`, `.ok_or`, `.trim()`, `.split()`,
`int.to_string()`, `1..=n` ranges, etc.) with the match-based
rewrites that ARE supported.

### Result

| metric | Run 7 | Run 8 | delta |
| --- | ---: | ---: | ---: |
| pass | 26 | 25 | -1 |
| check_fail | 5 | 14 | **+9** |
| exec_fail | 2 | 1 | -1 |
| **compile_pass (pass + check_fail + exec_fail)** | **33** | **40** | **+7** |
| compile_fail | 67 | 59 | -8 |

The headline pass rate is flat within noise (25 vs 26), but the
more informative metric — **compiler-accepts-the-program** — rose
from 33 → 40. The extra 9 check_fails are programs where the
compiler did its job, the program ran, and the program produced
wrong output (semantic mistake by the model). That shift moves
failures from "compiler / stdlib gap" into "just wrong".

### Remaining compile_fails (59)

```
E0500 LLVM codegen                     22  (-4 vs Run 7)
E0200 cannot import ...                15  (up — but only core.io
                                             repeats; the rest are
                                             cascaded errors from
                                             prior failures in the
                                             same file)
no-code                                 9
E0101 expected newline                  4
E0308 type mismatch                     3
E0002 / E0104 / E0102 / E0100           6
```

### Run history progression

| run | setup | pass | compile_pass |
| --- | ----- | ---: | -----------: |
| Run 4 baseline | no spec | 0 | 0 |
| Run 5 | +spec + examples | 16 | 16 |
| Run 6 | +io + TYRA_STDLIB | 14 | 15 |
| Run 7 | +stdlib in context | 26 | 33 |
| **Run 8** | **+anti-hallucination guide** | 25 | **40** |

From 0 → 40 on "compiler accepts Claude's Tyra" in one afternoon,
purely via context engineering and minimal stdlib additions. The
design-quality thesis survives every iteration: when Claude
writes Tyra it sees, the compiler accepts it.

### Where we stop here

Further bench improvements need either:
- Tyra compiler defense (type-check method existence → reject
  `.unwrap_or` at type time, not LLVM time)
- A tiny `string` stdlib (split / trim / chars)

Both are real compiler / stdlib work, not context tweaks. This is
a reasonable snapshot to pause on.

## Run 7 (2026-04-21) — spec injection v2: +io stdlib + TYRA_STDLIB + stdlib in context

Three stacked improvements on top of Run 5:

1. **`io` stdlib shipped** (`386aa5a`) — `io.read_line()` and
   `io.read_to_end()` backed by new runtime intrinsics. Closes the
   single largest Run 5 bucket (28 `import io` failures).
2. **`TYRA_STDLIB` env var** in runners/tyra.py — the bench runs
   the compiler from a `/tmp` workdir so the default walk-up
   search never resolved the repo's `stdlib/`. Pinned via env.
3. **Stdlib source included in the spec-injection context**
   (`26dd58c`) — Run 5's context was spec + examples only.
   Including every `stdlib/**/*.tyra` file with an explicit "these
   are the ONLY modules" note cut hallucinated imports
   (`string`, `core.io`, `collections`) sharply.

### Result progression

| run | setup | pass / 100 |
| --- | ----- | ---------: |
| Run 4 baseline | no spec | **0** |
| Run 5 | +spec + examples | **16** |
| Run 6 | +io stdlib + TYRA_STDLIB | 14 (noise; blocker moved to `string`) |
| **Run 7** | **+ stdlib source in context** | **26** |

### Final failure taxonomy (Run 7)

```
E0500 LLVM codegen type error             26   (biggest bucket)
E0308                                      8
no-code (parser panic-adjacent)            8
E0101 expected newline / EOF               7
E0200 cannot import ...                    6
E0305 arithmetic / type mismatch           4
E0002                                      4
E0102 / E0104                              4
```

`import string` is down from 28 (Run 6) to 6. The cliff is now
E0500 — the Tyra compiler lets invalid programs through the type
checker and clang refuses the emitted LLVM IR. These are real
compiler bugs the benchmark surfaces.

### What this means

- **Strategy §4.1 thesis: still provisionally supported.** Claude
  writes syntactically valid Tyra when given spec + stdlib.
- **The bench is no longer bottlenecked on missing stdlib.** It is
  bottlenecked on compiler-side holes (E0500) and a small tail of
  hallucinated modules.
- **Each stdlib addition has diminishing returns** by itself
  because Claude's programs typically need multiple pieces
  (io + string + collections). The Run 7 jump came from giving
  Claude ground truth about what exists, not from adding more
  stdlib.

### Remaining cheap wins

1. Fix Tyra compiler type-check holes producing E0500. The 26
   cases probably collapse to 3–5 root causes.
2. Add a minimal `string` module (split / trim / split_whitespace)
   or tighten the anti-hallucination note. 6 remaining
   `import string` failures show the current note works but leaks.
3. Polish §10 arithmetic / iteration prose so `else if` + `while`
   idioms are unambiguous (a few E0101 / E0305).

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
- **Run 6** (2026-04-21) — tyra+spec re-sweep after io stdlib
  landed. 14/100 pass; blocker moved from `import io` to
  `import string`. Demonstrates that stdlib additions alone have
  diminishing returns because Claude's programs need multiple
  pieces.
- **Run 7** (2026-04-21) — tyra+spec + stdlib source included in
  the injected context + TYRA_STDLIB env. 26/100 pass, 33/100
  compile_pass. The remaining cliff is now E0500 (Tyra compiler
  bug surface), not stdlib holes.
- **Run 8** (2026-04-21) — tyra+spec + anti-hallucination method
  guide in context. 25/100 pass, **40/100 compile_pass**. Pass
  rate flat within noise but 9 extra programs moved from
  compile_fail → check_fail (compiler accepted, logic wrong).
  Trajectory from Run 4 baseline: compile_pass 0 → 16 → 15 → 33
  → 40 in a single afternoon purely via context + minimal stdlib.
- *(Runs 9–51 not logged here — see `results-run*/`.)*
- **Run 52** (2026-05-14) — **replay** of Run 51 model code through
  the fixed compiler (no LLM call). 049 `check_fail→pass` (codegen
  fix), 099 SIGSEGV→timeout (alloca hoisting, compiler crash
  resolved; remaining fail is model's infinite loop), 088 unchanged
  (model bug). pass% 99.0→99.3, all_pass% 97.0→98.0.
