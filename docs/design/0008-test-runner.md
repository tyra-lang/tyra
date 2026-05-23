# ADR-0008: Test Runner Design (v0.2)

**Status**: Accepted  
**Date**: 2026-05-19  
**Author**: Tyra project

---

## Context

v0.2 adds `tyra test` as the official test runner. The design must:

1. Fit naturally into the existing `tyra` CLI (no separate binary)
2. Require no new language syntax (lexer/parser/AST changes deferred)
3. Integrate with the `Result<T, E>` error-handling model already in the language
4. Produce CI-friendly output

## Decision

### Discovery convention

- Files: `*_test.tyra` (any directory, recursive from the given path)
- Functions: `fn test_*() -> Result<Unit, String>` with no parameters
- Files must not contain `fn main` or top-level executable statements

### Execution model (v0.5.0: per-test process isolation)

The runner synthesizes a `fn main` that supports two modes via `sys.args()`:

- **Dispatch mode** (argv[1] = test function name): runs exactly one test,
  exits 0 on pass, 1 on fail. Used by the Rust runner for isolation.
- **All-tests mode** (no argv): runs all tests in TAP order (legacy/compat).

**Compile-once, exec-per-test:** For each `*_test.tyra` file, the runner:
1. Writes the synthesized source alongside the test file (same directory, so
   `import` resolution works without resolver changes).
2. Compiles once to a native binary (kept across all test runs for the file).
3. For each `test_*` function, spawns a subprocess passing the test name as
   argv[1]. Each subprocess is isolated: a panic/abort/OOM in one test does
   not affect subsequent tests.
4. Aggregates per-subprocess TAP lines into file-level TAP output.
5. Deletes the binary and the synthesized source after all tests finish.

TAP output format is identical to earlier versions:

```
TAP version 14
1..3
ok 1 - test_addition
not ok 2 - test_subtraction
# expected 5, got 3
ok 3 - test_multiplication
```

### Failure semantics

- A test function returning `Err(msg)` → `not ok` (subprocess exits with code 1)
- A subprocess exiting with a non-zero code (panic, abort, OOM, signal) →
  `not ok`; subsequent tests in the same file still run (isolation guarantee).
- Timeout kills the subprocess; remaining tests are also reported as `not ok`
  with a `(timeout after Ns)` annotation.

### Assertion API

`stdlib/assert.tyra` provides concrete-typed helpers:

| Function | Signature |
|---|---|
| `eq` | `(Int, Int) -> Result<Unit, String>` |
| `eq_str` | `(String, String) -> Result<Unit, String>` |
| `eq_bool` | `(Bool, Bool) -> Result<Unit, String>` |
| `ne` | `(Int, Int) -> Result<Unit, String>` |
| `ne_str` | `(String, String) -> Result<Unit, String>` |
| `is_ok` | `(Result<Unit, String>) -> Result<Unit, String>` |
| `is_err` | `(Result<Unit, String>) -> Result<Unit, String>` |

Generic `assert.eq<T>` requires trait bounds not available in v0.2; typed
variants are used instead.

## Alternatives considered

### A. `test "name" do ... end` language syntax

Rejected for v0.2: requires lexer, parser, AST, resolver, MIR, and codegen
changes across six crates. The value-add over the convention-based approach is
cosmetic. Deferred to a separate ADR.

### B. Separate `tyra-test` binary

Rejected: fragments the toolchain. The Go model (single binary, all
subcommands) is a stated Tyra design goal.

### C. Subprocess per test function

Deferred in v0.2 due to startup overhead and process-spawning API maturity.
**Implemented in v0.5.0** as the primary execution model (see above). The
synthesis approach (compile once, exec per test using `sys.args()` dispatch)
minimises per-test overhead to one process spawn without any new language syntax.

## Consequences

- **v0.5.0**: Per-test subprocess isolation ships. A panicking test no longer
  kills sibling tests in the same file — each test runs in its own process.
- `assert.panics` is still out of scope: it requires a way to assert on crash
  semantics (signal, exit code) that is not yet in the stdlib. Now that
  isolation is available, `assert.panics` can be implemented in a future ADR.
- The `test "name"` syntax remains available as a future ADR.
- TAP output is parseable by most CI systems (pytest-tap, tap-junit, etc.)
  without any Tyra-specific plugin.
