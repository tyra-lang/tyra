# Developer Tooling Roadmap

**Last updated**: 2026-05-19

This document tracks the status and future direction of `tyra fmt`, `tyra test`,
and related developer experience tooling.

---

## Shipped in v0.3.0 (2026-05-19)

### `tyra fmt`

- `--check` mode: exits 1 if any file would change (CI-friendly)
- `--stdin` mode: reads from stdin, writes formatted source to stdout (editor/pipe integration)
- Line-length enforcement: 100-column limit with one-param-per-line wrapping for long signatures
- Idempotent: formatting an already-formatted file is a no-op
- Preserves all comments (standalone, inline on statements, inline on item headers)
- 2-space indentation

### `tyra test`

- `--filter <pattern>`: substring match on test function names to run a subset
- `--list [--filter]`: list matched test functions without executing
- `--format tap`: TAP version 14 (default); includes `# time: <s>s` per file
- `--format junit`: JUnit-compatible XML (`<testsuites>`/`<testsuite>`/`<testcase>` with `time=`)
  - Infrastructure failures (compile errors) produce a synthetic suite — no silent green in CI
- Non-zero binary exit (panic, OOM, abort) always counted as at least one failure
- `import assert` provides typed assertion helpers (`eq`, `ne`, `eq_str`, `is_ok`, `is_err`, etc.)

### `tyra new`

- `tyra new <name> [--lib] [--vcs none]` — scaffold a bin or lib project
- Generates `Tyra.toml`, `src/<name>.tyra`, `.gitignore`, `README.md`
- `--lib` emits `export fn` template; `--vcs none` suppresses `.gitignore`

### `tyra mod`

- `mod init/add/update/remove/show/tree/sync/clean` — full dependency lifecycle
- Path and git+rev dependencies; git deps cached in `~/.tyra/cache/`
- `mod sync --check/--json/--quiet` for CI use
- `mod show [--json]`, `mod tree [--json]` for machine-readable output
- Three-layer import resolution (ADR 0010): local `src/` → deps → stdlib; E0217 on ambiguity

### `tyra bench`

- `tyra bench ai-gen [options]` — AI generation quality benchmark (delegates to `bench/ai-gen/harness.py`)

---

## Shipped in v0.2.0 (2026-05-19)

### `tyra fmt` (v0.2.0 baseline)

- Formats a single `.tyra` file or an entire directory (recursive)
- Preserves all comments; 2-space indentation; idempotent

### `tyra test` (v0.2.0 baseline)

- Discovers `*_test.tyra` files from the given path (default: current directory)
- TAP version 14 output; E0216 enforced

---

## Near-term (v0.4.0)

- Lambda C ABI and generic `List<T>` (`map`/`filter`/`fold`)
- Generic `assert.eq<T>` via type-class dispatch
- `tyra bench <dir>` — general-purpose wall-clock microbenchmark runner

## Medium-term (v0.5+)

- `test "name"` language syntax (requires separate ADR)
- `assert.panics` — requires per-test subprocess isolation
- SemVer resolver, `Tyra.lock`, package registry (`tyra publish`)

## Long-term

- VS Code test explorer integration via LSP test protocol
- DAP debugger integration
- Coverage reporting (`tyra test --coverage`)
- Cross-compilation support
