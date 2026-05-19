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

### Execution model

The runner synthesizes a `fn main` that calls each `test_*` function in order
and prints TAP (Test Anything Protocol) version 14 output to stdout:

```
TAP version 14
1..3
ok 1 - test_addition
not ok 2 - test_subtraction
# expected 5, got 3
ok 3 - test_multiplication
```

The synthesized runner is written alongside the test file as a temporary
`__tyra_test_runner_<pid>.tyra` and deleted after execution. Writing it to the
same directory ensures `import` resolution (stdlib and local modules) works
correctly without any changes to the resolver.

### Failure semantics

- A test function returning `Err(msg)` → `not ok`
- The synthesized binary exiting with a non-zero code (panic, abort, OOM) is
  always treated as at least one failure, regardless of how many TAP lines were
  emitted before the crash.

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

Rejected for v0.2: eliminates the "binary crash = failure" ambiguity but adds
significant startup overhead and requires a process-spawning API not yet
stabilized. Deferred to v0.2.x or v0.3.

## Consequences

- `assert.panics` is out of scope: Tyra's `panic` returns `Never`, so a
  panicking test brings down the entire synthesized runner process. Per-test
  subprocess isolation is the prerequisite; deferred.
- The `test "name"` syntax remains available as a future ADR.
- TAP output is parseable by most CI systems (pytest-tap, tap-junit, etc.)
  without any Tyra-specific plugin.
