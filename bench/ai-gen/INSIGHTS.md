# ai-gen — early findings

First real data from `bench/ai-gen/harness.py`. This document records
qualitative observations that raw pass/fail counts in `SUMMARY.md` hide.
It is intended as a running log, updated as more runs complete.

## Run 2 (2026-04-21) — 39 prompts × 5 languages × Codex CLI

Combined 9-prompt smoke + 30-prompt sweep. Raw and adjusted tables
below. Adjusted rate excludes `generator_fail` because those are
Codex CLI timeouts / throttling, not a language-quality signal.

### Raw (includes generator_fail)

| language | pass | adjusted% raw | notes |
| -------- | ---- | ------------- | ----- |
| ruby     | 20 / 39 | 51.3% | — |
| crystal  | 10 / 30 | 33.3% | — |
| v        |  8 / 30 | 26.7% | — |
| gleam    |  6 / 30 | 20.0% | — |
| tyra     |  3 / 39 |  7.7% | — |

### Adjusted (excludes generator_fail — Codex timeouts)

| language | pass / tried | adjusted% | gap vs ruby |
| -------- | ------------ | --------- | ----------- |
| ruby     | 20 / 21 | 95.2% | — |
| crystal  | 10 / 12 | 83.3% | -11.9 |
| v        |  8 / 12 | 66.7% | -28.5 |
| gleam    |  6 / 11 | 54.5% | -40.7 |
| tyra     |  3 / 18 | 16.7% | -78.5 |

### What's real in these numbers

**Ruby → Crystal → V → Gleam → Tyra ordering is stable.** The ordering
survives the generator_fail adjustment and is consistent with corpus
presence on public code hosts. Codex knows Ruby best; Gleam less well
than Ruby/Crystal/V; Tyra not at all (zero public Tyra code existed
before this project).

**Tyra's gap is dominated by compile_fail, not model refusal.** In
the 18 non-timeout Tyra attempts, 13 compile_fail vs 2 check_fail
vs 3 pass. The model writes what it thinks is Tyra, and the compiler
rejects it. That is the exact failure mode strategy.md §4.1 predicted.

**Codex throttling collapsed the tail of the run.** The last 3 prompts
(039 specifically) returned `generator_fail` across all 5 languages —
clearly a rate-limit or quota event, not a language-specific issue.
Each language in the sweep absorbed ~18 `generator_fail`s, roughly
evenly. Any single-run ranking that includes these will overstate
the languages the timeout happened to skip fewer times.

### Tyra compile_fail taxonomy (13 cases)

From sampling the failing results:

1. **`import fs` for stdin.** Multiple prompts that read stdin
   triggered Codex to write `import fs` — but fs is file-only in
   v0.1 (§17.3.1). No stdin-reading stdlib module exists yet. This
   is the single largest loss driver. An `io` module with
   `read_line` / `read_to_end` would cost little to spec and
   recover many prompts. Tracked as "stdlib gap #1."
2. **Modulo workarounds.** Tyra v0.1 has no `%` operator. Codex
   reached for `value - (value / divisor) * divisor == 0` or
   similar, and the parser rejected the expression in an `else if`
   chain — possibly a precedence / form issue worth reproducing.
3. **Mixed identifier conventions.** Codex sometimes produces
   `camelCase` identifiers where Tyra parsing expected snake_case;
   lexer/parser error surfaces downstream.

None of these are semantic model errors. They are all "the model
used a construct the v0.1 stdlib or syntax doesn't provide." A
small stdlib + syntax expansion addresses the bulk of them.

### Gleam's compile_fail modes (5 cases)

Gleam's project structure (gleam.toml + src/) adds friction: Codex
sometimes wrote `gleam run` -ready top-level code that Gleam's
module system rejected. These are structural rather than semantic
errors too.

### V's compile_fail modes (3 cases)

V is more forgiving; the losses were specific function signature /
type mismatches rather than framework issues.

### Crystal's compile_fail modes (2 cases)

Smallest loss count outside Ruby. Crystal's Ruby-descended surface
is familiar to Codex and produces mostly-correct code.

### Run-level methodology notes

1. **Single seed**. One sample per (prompt, lang). Multiple seeds
   would tighten confidence intervals; headlines should average ≥ 3.
2. **Codex-only**. Claude generator is implemented but
   `ANTHROPIC_API_KEY` was unset; re-running with Claude is the next
   big experiment.
3. **Codex model unknown**. The harness records the CLI version
   (`codex-cli 0.120.0`); the actual model is whatever the local
   config uses.
4. **60 prompts remain unrun** (041-100). After the throttling tail
   this run stopped at 039. A follow-up sweep is needed for the
   full 100.

## What this tells us about strategy.md §4.1

Tyra's thesis — "AI-generated Tyra code compiles more reliably than
alternatives because the design is more constrained" — is not yet
supported by the data. The design side of the thesis may still be
true, but its benefit is swamped by **stdlib absence**. The model
cannot pick Tyra's safe idioms over unsafe alternatives if Tyra
does not expose the right idioms at all.

Concrete implication: the roadmap item "Tier 1 stdlib stable" (§6.2)
is not just feature work — it is the prerequisite for the AI
benchmark to produce a fair answer. An `io` stdin module and a
documented `%` (or rem/mod function) would be the two cheapest wins.

## Run 1 (2026-04-21) — baseline, Tyra + Ruby only

Kept for reference. 9 prompts × 2 languages × Codex. Ruby 9/9, Tyra
2/9. Subsumed by Run 2. See earlier version of this file in git
history for the pre-Crystal/V/Gleam detail.
