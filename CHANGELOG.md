# Changelog

All notable changes to Tyra are documented here.

Format: `## [version] - YYYY-MM-DD` with sections **Stable**, **Experimental**,
**Known Limitations**, **Not in This Release**.

---

## [0.2.0] - 2026-05-19

### Stable

**Language**
- `continue` statement — transfer control to the next loop iteration (`while`/`for` only; E0215 outside a loop)

**Standard library**
- `assert` module: `eq`, `eq_str`, `eq_bool`, `ne`, `ne_str`, `is_ok`, `is_err` — all return `Result<Unit, String>` for use with `?`

**Compiler and toolchain**
- `tyra fmt [--check] <file|dir>` — format Tyra source in-place; `--check` exits 1 if any file would change; accepts a directory (recursive)
- `tyra test [path]` — discover and run `*_test.tyra` files; TAP-compatible output; exits 1 if any test fails
  - Discovers `fn test_*() -> Result<Unit, String>` functions automatically
  - Synthesizes a test runner without requiring language-level test syntax
  - Non-zero binary exit (panic, abort) is always counted as a failure

**Runtime**
- FFI string ownership fixed: all functions returning strings to Tyra now allocate via `GC_malloc_atomic` instead of `CString::into_raw`, eliminating the long-running string leak
- Float display: `to_string` on integer-valued floats now preserves `.0` (e.g. `0.0` instead of `0`)

### Known Limitations

- **Windows**: untested. Build via WSL2 is recommended.
- **`tasks.select` literal-only**: `tasks.select([t1, t2])` accepts list literals only.
- **Task handles in `for` / `match`**: use index access or `tasks.join_all` instead.
- **No package manager**: dependency management is not yet available.
- **Breaking changes**: expect breaking changes before v1.0.

### Not in This Release

- Pre-built binaries (homebrew, apt, etc.) — planned for a later release
- VS Code Marketplace publication — planned for a later release
- `tyra mod` / `tyra new` — planned for a later release
- Package manager — planned for a later release
- Generic `List<T>`, `map` / `filter` / `fold` — requires lambda C ABI; deferred
- `Set<T>` — deferred
- `test "name"` language syntax — deferred (separate ADR)

---

## [0.1.0] - 2026-05-17

### Stable

**Language core**
- Type inference, algebraic data types (ADT), exhaustive `match`
- `Result<T, E>`, `Option<T>`, `?` propagation operator
- `async` / `await` / `spawn`
- Value types (`value`), reference types (`data`), traits (`trait`)
- String interpolation (`#{expr}`)
- `for` / `while` / `break` / `if` / `else`
  - Note: `continue` is not in v0.1 per language spec §5.2

**Standard library**
- `string`: len, is_empty, trim, to_upper, to_lower, contains, starts_with, ends_with, parse_int, byte_at, substring, reverse, from_byte, split, split_whitespace
- `list` (List<Int> only): len, get, push, sum, max, min, contains, index_of
  - Note: map/filter/fold require lambda ABI not yet available; deferred to v0.2
- `fs`: read_to_string, write_string, exists
- `io`: read_line, read_to_end
- `float`: eq, approx_eq, abs, floor, ceil, round, min, max, to_string, parse, from_int, to_int, is_nan, is_infinite
- `json`: parse; Value methods: kind, as_string, as_int, as_bool, get (by key), at (by index)

**Compiler and toolchain**
- `tyra check` — type-check without codegen
- `tyra run <file.tyra>` — compile and run
- `tyra build <file.tyra> [-o output]` — compile to native binary
- LLVM codegen with Boehm GC runtime (macOS arm64, Linux x86_64)
- Panic-converted-to-diagnostic: internal errors print as `error[Exxxx]`, not backtraces

**LSP and editor**
- `tyra-lsp` language server: diagnostics, hover, go-to-definition, completion, find references, rename, signature help, semantic tokens, inlay hints, and more
- VS Code extension: development install via F5

**Testing and quality**
- 11-program static conformance corpus (`bench/static-corpus/`)
- Negative corpus: 9 expected-error programs (`bench/static-corpus/bad/`)
- Spec coverage report (`bench/static-corpus/coverage.sh`)
- CI: static corpus check on every push/PR to `main`
- Benchmark run 53: 99.3% pass rate (142/143 generated programs correct)

**Documentation**
- [Getting Started guide](docs/getting-started/README.md) (7 chapters, ~30 min)
- Language specification v0.1 (Japanese authoritative, English translation)
- Architecture decision records (`docs/design/`)

### Experimental

- **`http.server` stdlib**: basic single-threaded GET/POST routing. No TLS, no middleware, no production hardening. Use for local development and demos only.

### Known Limitations

- **String GC**: allocated strings are never reclaimed by the garbage collector. Acceptable for short-lived CLI programs; avoid long-running servers.
- **Windows**: untested. Build via WSL2 is recommended.
- **Float display precision**: uses Rust's `Display`, which may print unexpected representations for edge values (e.g., `0` instead of `0.0`).
- **`tasks.select` literal-only**: `tasks.select([t1, t2])` accepts list literals only; a dynamic `List<Task<T>>` variable is rejected at compile time.
- **Task handles in `for` / `match`**: iterating over a task list with `for t in tasks` or binding a task in a match pattern drops the task-result type; use index access or `tasks.join_all` instead.
- **No formatter**: `tyra fmt` is not yet available.
- **No test runner**: `tyra test` is not yet available.
- **No package manager**: dependency management is not yet available.
- **Breaking changes**: expect breaking changes before v1.0.

### Not in This Release

- Pre-built binaries (homebrew, apt, etc.) — planned for v0.2
- VS Code Marketplace publication — planned for v0.2
- `tyra fmt` formatter — planned for v0.2
- `tyra test` test runner — planned for v0.2
- Package manager — planned for a later release
