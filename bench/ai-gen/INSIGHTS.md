# ai-gen — early findings

First real data from `bench/ai-gen/harness.py`. This document records
qualitative observations that raw pass/fail counts in `SUMMARY.md` hide.
It is intended as a running log, updated as more runs complete.

## Run 1 (2026-04-21) — 9 prompts × {Tyra, Ruby} × Codex

| language | pass | fail breakdown |
| -------- | ---- | -------------- |
| ruby     | 9/9  | — |
| tyra     | 2/9  | 3 compile_fail, 3 generator_fail (codex timeout @ 180s), 1 check_fail |

### Tyra failure taxonomy

**compile_fail — stdlib gaps surfaced by the model's first-instinct code**

- `001-fizzbuzz`, `006-factorial`: Codex wrote `import fs` expecting a
  stdin reader. Tyra's `fs` is file-only (§17.3.1); no stdin module
  ships in v0.1. The model has no way to know this because the spec
  is not yet in common crawl corpora.
- `002-word-count`: same `import fs` pattern for stdin.
- Modulo workaround: Codex wrote
  `value / divisor * divisor == value` as a `%`-free divisibility
  check; the parser rejected it at the `==` position, suggesting
  a precedence / form issue in `else if` chains. Worth verifying
  with a minimal repro.

**generator_fail — Codex timeouts (180s)**

- `007-gcd`, `008-reverse-string`, `009-palindrome`: Codex spent the
  full wallclock budget and returned nothing. Pattern: the three
  slowest generations are all Tyra, never Ruby. Most likely cause:
  the model reasons longer when it does not recognize the language
  and the prompt keeps it in exploration mode. Raising the timeout
  to 300s and measuring again is the next step.

**check_fail — 003-option-chain**

- Produced `OK=0NONE` on one line instead of `OK=5\nNONE` on two.
  Compiled cleanly; the model made a semantic mistake around
  integer division and newline handling. This is the only failure
  mode where the toolchain is healthy and the model simply got the
  answer wrong.

### What this tells us about strategy.md §4.1

The headline claim Tyra needs to defend — "AI-generated Tyra code
compiles more reliably than AI-generated Crystal/V/etc." — cannot
be measured with this data yet; it compares Tyra to Ruby only, and
the comparison is the wrong direction (Ruby wins 100%). The 5-
language comparison across 100 prompts is the test that matters.
Run 2 below.

## Run 2 — 30 prompts × 5 languages × Codex (in progress)

Kicked off after Crystal / V / Gleam installed via brew. Results
will populate `results/` asynchronously. Expected wall time at
~45s/run × 150 runs ≈ 2 hours. The updated SUMMARY.md and a new
section here will follow when that run completes.

## Known caveats that will color the numbers

1. **Single seed per (prompt, lang, gen)**. Frontier models are
   sampled, so one run is not a reliable signal. The harness supports
   `--seed N`; for publishable headlines average ≥ 3.
2. **Codex chooses its own model** (`~/.codex/config.toml`). When we
   add Claude, the delta is model × language, not language alone.
3. **The prompts do not hint stdlib**. For Tyra this is brutal — the
   model has no prior exposure to tyra stdlib. For Ruby / Crystal / V
   this is irrelevant. This is the benchmark we care about (Tyra's
   thesis is that AI code works *without* hand-holding); we note it
   so the asymmetry is not mistaken for a harness bug.
4. **`ruby -c` is a syntax check, not a type check**. Ruby's
   "compile" stage is weaker than the static languages'; a Ruby run
   that reports `pass` may still contain bugs that a typed language's
   compile stage would reject. When aggregating, treat the Ruby
   compile stage as a floor.
