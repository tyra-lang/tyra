# Benchmark Methodology

This document explains how the ai-gen benchmark is designed to produce
results that are fair, reproducible, and auditable.

## Prompt neutrality

Every prompt is a plain English description of a programming task. No
prompt mentions any language, framework, stdlib function, or syntax
hint. The same description is given verbatim to all six language
runners.

**Audit procedure:** The full prompt set is in `prompts/*.yaml`. Any
reader can inspect every file and verify that no language-specific
terminology appears in the `description` field. The `tags` field is
metadata only and is not shown to the model.

Prompts are versioned in git. Any change to a prompt description is a
breaking change that requires a new full sweep (not a patch to
existing results).

## Scoring criteria

A run is scored `pass` if and only if all three stages succeed:

1. **generate** — the model returns a non-empty string that the runner
   can write to a source file.
2. **compile** — the compiler exits 0 (for Ruby, `ruby -c` exits 0).
3. **execute** — the binary exits 0, every string in
   `stdout_must_contain` appears in stdout, and no string in
   `stdout_must_not_contain` appears.

Partial success does not exist: a run that compiles but executes
incorrectly is `exec_fail`, not a partial pass.

**Why markers, not exact equality:** Verifying exact stdout would
require 100 reference implementations per language. The marker
approach is weaker than full correctness but strictly stronger than
"it compiled," and is uniform across languages with different
formatting conventions.

## Model pinning

**claude generator:** `config.yaml` has `model: null` by default,
which lets `claude -p` use whichever model the user's CLI is
configured for. Set `model: claude-sonnet-4-6` (or any model ID) in
`config.yaml` to pin a specific version for reproducible sweeps.
The model name is recorded in each result JSON for traceability.

Example `config.yaml` pin:

```yaml
generators:
  claude:
    model: "claude-sonnet-4-6"
```

**codex generator:** The `codex` CLI chooses its own model. The
harness cannot override it. The model name is recorded from
`codex --version` output, but pinning a specific codex model is not
supported by the CLI interface. Published results note this
limitation.

**Recommendation:** Set an explicit `model` in `config.yaml` before
running a sweep intended for publication.

## Seed policy

Each seed produces an independent model sample for the same prompt.
Running N seeds gives N independent pass/fail verdicts per
(prompt, language, generator) triple.

A single seed gives a point estimate. For headline numbers used in
public comparisons, run ≥3 seeds so the variance is visible.

The seed value is not passed to the model API — it is used only to
disambiguate result filenames when multiple runs of the same
(prompt, lang, gen) are stored.

## Threats to validity

| Threat | Mitigation |
|---|---|
| Prompt phrasing favours some languages | Prompts are kept at the algorithmic level; no library or idiom hints. Open to review in `prompts/*.yaml`. |
| Model non-determinism | Record raw generated code alongside pass/fail; aggregate over ≥3 seeds for publication. |
| Gleam project model mismatch | Runner wraps code in a template project. Results may underestimate Gleam's true capability; noted in every summary. |
| Compiler version differences | Compiler version recorded in runner metadata per result JSON. |
| Tyra compiler version not pinned | Use `git checkout <tag>` + `cargo build --release` to reproduce; version embedded in result JSON via `tyra --version`. |
| Codex model not pinnable | Noted in results and in this document. |
| Marker coverage too loose | Markers cover the core observable output. False positives are possible but unlikely at 100-prompt scale. |

## What this benchmark does not measure

- **Full functional correctness** — a program that outputs the right
  markers but performs wrong computation passes.
- **Code quality** — no style, maintainability, or security grading.
- **Performance** — no wall-clock or memory measurements.
- **Human-written baseline** — this benchmark measures AI generation
  only; a human expert baseline is not included.
