# Developer Tooling Roadmap

**Last updated**: 2026-05-23

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
- `--list [--filter]`: list matched test functions without executing; output order is stable: files in lexicographic path order, functions in source declaration order
- `--format tap`: TAP version 14 (default); includes `# time: <s>s` per file
- `--format junit`: JUnit-compatible XML (`<testsuites>`/`<testsuite>`/`<testcase>` with `time=`)
  - Infrastructure failures (compile errors) produce a synthetic suite — no silent green in CI
  - Compile-error diagnostics are propagated into `<failure message="…"/>` (v0.5.0+)
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

## Shipped in v0.4.0 (2026-05-22)

- Lambda C ABI and closures (spec §9.4, ADR 0011) — first-class `fn(T)->U` values with captured environments
- Generic `List<T>` + `map`/`filter`/`fold` (spec §17.3.5) — `List<Int>` and `List<String>` fully supported
- Generic `assert.eq<T>` / `assert.ne<T>` — overloaded for `Int`, `String`, `Bool` via compiler-known `Eq` ability
- `tyra bench <dir>` (spec §18.8) — general-purpose wall-clock microbenchmark runner for `*_bench.tyra` files
- `tyra test --timeout <secs>` and `--jobs N` (parallel test execution with deterministic output)
- `Tyra.lock` + floating `branch` constraints + transitive dependency resolution (minimal solver)

> **v0.4.0 / v0.5+ scope boundary:** `Tyra.lock` in v0.4.0 covers lockfile
> generation/reading and a *minimal solver* (floating `branch` constraint
> interpretation + transitive dependency merge, `--locked` CI mode). A full
> *registry-backed resolver* (candidate fetching from a central registry) and
> `tyra publish` remain v0.5+.

## Shipped in v0.5.0 (2026-05-23)

- **Per-test process isolation** in `tyra test`: each `test_*` function now
  runs in its own subprocess. A panic/abort/OOM in one test no longer
  prevents sibling tests from running. TAP output format is unchanged.
  Implementation: compile-once + `sys.args()` argv dispatch per test.
  New public API in `tyra-driver`: `RunOutcome` and `run_binary`.

## Shipped in v0.6.0 (2026-05-25)

- **`test "name"` language syntax** (ADR-0013): contextual-keyword item syntax with optional `panics` modifier; body lowers to `Result<Unit, String>`
- **Panic expectation** (ADR-0012): runner-native; `exit(101)` + `__TYRA_PANIC__` sentinel identifies intentional panics; OOB = `exit(102)`; no false-pass from segfault/OOM
- **`tyra test --coverage`** (ADR-0014): Tyra-native line/function coverage; per-test `.covraw` files written via atexit handler, merged by parent; branch coverage out of scope
- **`time` and `log` stdlib modules**: `now_unix`, `monotonic_millis`, `log.info/warn/error`
- **Generic `Map<K,V>` and `Set<T>`** (ADR-0015): full generalization with boxed erased-value ABI + compiler-emitted `eq`/`hash` fn pointers
- **DAP debugger** (ADR-0014 Phase 4): DWARF in LLVM IR text, `lldb-dap` adapter, VS Code `debuggers`/`breakpoints` contributions

## Near-term (v0.6+)

- Type-checker ergonomics / E0308 diagnostic improvements (highest-leverage AI-audibility improvement)
- Full registry-backed SemVer resolver and package registry (`tyra publish`)

## Long-term

- VS Code test explorer integration via LSP test protocol
- inkwell migration (replaces hand-written LLVM IR text)
- Branch coverage
- Cross-compilation support
