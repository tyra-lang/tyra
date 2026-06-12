# ADR 0026: Machine-readable diagnostics — `--error-format json`

- **Status**: Accepted
- **Date**: 2026-06-12
- **Spec sections affected**: none (CLI surface only; §18 toolchain docs get a reference)

## Context

Tyra's primary differentiation claim is AI-auditability: an LLM agent should be
able to generate Tyra code, check it, read the diagnostics, and fix its own
mistakes. Today `tyra check` renders diagnostics only as human-oriented text.
An agent must scrape that text, which is fragile (multi-line spans, i18n via
`TYRA_LANG=ja`, future format changes).

The ai-gen failure triage (2026-06-12, `bench/ai-gen/INSIGHTS.md`) confirmed the
self-correction loop as the primary agent use case: generate → check → parse
diagnostics → fix → run.

Constraint from the maintainer (plan review, 2026-06-12): a machine-readable
mode is useless unless the output stream is machine-readable **on every path**,
including usage errors, missing files, dependency-resolution failures, and
internal errors. A single stray human-oriented line breaks downstream parsers.

## Decision

Add `--error-format json` to **`tyra check` and `tyra build` only**.

1. **Commands**: `check` and `build`. `tyra run` is excluded in v0.11 — the
   user program's own stdout/stderr would interleave with diagnostics. The
   agent loop is: generate → `check --error-format json` → fix → `run`.
2. **Stream**: all records go to **stderr**. stdout keeps its existing
   behaviour (build artifact messages, nothing for check).
3. **Format**: NDJSON — one JSON object per line. Each line parses
   independently, so partial output (crash, kill) remains parseable.
4. **Stream purity**: with `--error-format json`, stderr carries **NDJSON
   records only, on all code paths**. Non-diagnostic failures are wrapped in
   an `error` record instead of free text.
5. **Record types** (3):

   ```json
   {"type":"diagnostic","code":"E0305","severity":"error","message":"...",
    "spans":[{"file":"main.ty","line":10,"col":16,"end_line":10,"end_col":28,
              "label":"type mismatch"}],
    "help":"..."}
   {"type":"error","kind":"file-not-found","message":"..."}
   {"type":"summary","errors":1,"warnings":0}
   ```

   - `diagnostic` — compiler diagnostics rendered from `tyra-diagnostics`.
     `help` is optional; `spans` may be empty for span-less diagnostics.
   - `error` — non-diagnostic failures (usage error, file not found,
     dependency resolution failure, internal error). `kind` is a stable
     lower-kebab-case discriminator.
   - `summary` — always the **last line** on every exit path. Its presence
     tells the consumer the stream terminated normally (not truncated).
6. **Exit codes**: unchanged from text mode (0 = no errors, 1 = errors).
7. **i18n**: `message`/`label`/`help` honour `TYRA_LANG` exactly like text
   mode. `code`, `type`, and `kind` are locale-independent; agents should
   dispatch on those.
8. **Stability**: the record schema is append-only after v0.11.0 — new fields
   may be added, existing fields are not renamed or removed.

### Implementation note — stderr centralisation

The guarantee in (4) requires auditing every `eprintln!`/render call reachable
from `check`/`build` in `tyra-cli` and `tyra-driver`, and routing them through
a single sink that knows the active format. The sink is selected once at CLI
argument parsing; anything that prints before argument parsing completes
(e.g. clap usage errors) must also be wrapped — clap errors are caught and
re-emitted as `{"type":"error","kind":"usage"}`.

## Consequences

- Agents and editors get a stable, parseable interface; the self-correction
  loop no longer depends on scraping localized text.
- Every new failure path added to `check`/`build` must go through the sink —
  enforced by a regression test that runs failure scenarios (missing file,
  bad flag, unresolved dependency) and asserts stderr is pure NDJSON.
- `tyra run` users do not get JSON diagnostics in v0.11; extending to `run`
  (e.g. diagnostics on stderr only until exec starts) is future work.
- The NDJSON schema becomes a compatibility surface (append-only).

## Alternatives considered

| Option | Rejected because |
|---|---|
| Single JSON array document | Not incrementally parseable; truncated output is unrecoverable |
| JSON to stdout | stdout already has meaning for `build`; mixing breaks pipelines |
| Cover `tyra run` too | User program output interleaves with diagnostics on stderr; deferred |
| rustc-compatible schema | Larger surface than needed; Tyra-specific `kind`/`summary` records wanted |
| Leave non-diagnostic errors as text | Violates stream purity; one stray line breaks every consumer |
