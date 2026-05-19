# Developer Tooling Roadmap

**Last updated**: 2026-05-19

This document tracks the status and future direction of `tyra fmt`, `tyra test`,
and related developer experience tooling.

---

## Current state (v0.2.0)

### `tyra fmt`

- Formats a single `.tyra` file or an entire directory (recursive)
- `--check` mode: exits 1 if any file would change (CI-friendly)
- Preserves all comments: standalone, inline on statements, inline on item
  headers (fn, import, type alias, ADT, trait, impl)
- Idempotent: formatting an already-formatted file is a no-op
- 2-space indentation, line width 100

### `tyra test`

- Discovers `*_test.tyra` files from the given path (default: current directory)
- Runs functions matching `fn test_*() -> Result<Unit, String>` with no parameters
- TAP version 14 output — parseable by standard CI tap-reporters
- Non-zero binary exit (panic, OOM, abort) always counted as at least one failure
- `import assert` provides typed assertion helpers (`eq`, `ne`, `eq_str`, etc.)

---

## Near-term (v0.2.x)

- `tyra fmt`: line-length enforcement with argument-list wrapping
- `tyra test`: `--filter <pattern>` to run a subset of tests
- `tyra test`: timing output per test function

## Medium-term (v0.3)

- `test "name"` language syntax (requires separate ADR)
- `assert.panics` — requires per-test subprocess isolation
- `tyra bench` — microbenchmark runner (criterion-style)
- `tyra mod` — module/package management
- `tyra new` — project scaffolding

## Long-term

- VS Code test explorer integration via LSP test protocol
- DAP debugger integration
- Coverage reporting (`tyra test --coverage`)
