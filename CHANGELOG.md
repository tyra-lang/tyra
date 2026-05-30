# Changelog

All notable changes to Tyra are documented here.

Format: `## [version] - YYYY-MM-DD` with sections **Stable**, **Experimental**,
**Known Limitations**, **Not in This Release**.

---

## [0.8.0] — Lexical Bengio (2026-05-30)

### Stable

**Hindley-Milner type inference — rank-1 unification (ADR-0020)**
- Added `TyVarId(u32)` and `Substitution(HashMap<TyVarId, Ty>)` as the rank-1 HM inference foundation.
- `unify(a, b, &mut subst)` with occurs check prevents infinite types.
- `types_compatible()` delegates to `unify()` internally; no semantic change to existing programs.
- `check_no_type_errors()` guard added in `tyra-driver` before LLVM IR emission: `Ty::Error` or unresolved `Ty::Var` reaching codegen now emits **`E9001 InternalTypeLeakedToCodegen`** and exits cleanly (exit code 1) instead of crashing LLVM with an opaque IR type-mismatch error.
- `E9001` is the first entry in the `E9xxx` ICE (Internal Compiler Error) range reserved in `tyra-diagnostics`.
- **E0500 occurrences in AI-gen benchmark: 0** (was 1 in Run 17). Run 18 result: 86/100 pass (seed=18); cross-seed comparison with Run 17 seed=2 is not direct — see `bench/ai-gen/results/SUMMARY.md` for details.

**E0308 heuristic (iv) — ADT variant suggestion**
- When a variant name (e.g. `Red`) is used where its parent ADT type (e.g. `Color`) is expected, E0308 now appends `help: did you mean \`Color.Red\`?`.
- Suppressed when the same variant name appears in two or more ADTs (no false positives).
- Implemented via `TypeEnv.variant_to_adts` reverse map populated at `register_adt()` time.

**LinkedMap / LinkedSet — insertion-order-preserving persistent collections (ADR-0019)**
- `LinkedMap<K,V>`: insertion-order entries array + HAMT key-index hybrid. API: `new()`, `insert(k,v)`, `get(k) -> Option<V>`, `remove(k)`, `contains_key(k)`, `len()`.
- `LinkedSet<T>`: symmetric wrapper. API: `new()`, `insert(v)`, `contains(v)`, `remove(v)`, `len()`.
- `for k, v in lm { ... }` and `for v in ls { ... }` iterate in insertion order — **guaranteed by spec §11**.
- HAMT-based `Map<K,V>` / `Set<T>` are unchanged; `LinkedMap` / `LinkedSet` are independent types with separate intrinsics (`tyra_linked_map_*` / `tyra_linked_set_*`).
- `import linked_map` / `import linked_set` to use; no literal syntax in v0.8.
- Runtime conformance tests: `bench/static-corpus/linked_map_test.tyra` (4 tests), `bench/static-corpus/linked_set_test.tyra` (3 tests).

**Windows native support — MSVC ABI (ADR-0021)**
- `vcpkg.json` manifest: `bdwgc` (Boehm GC) via `x64-windows` (MSVC dynamic) triplet.
- `tyra-driver`: Windows linker path uses `llc.exe` (IR → COFF obj, `-mtriple=x86_64-pc-windows-msvc`) + `lld-link.exe` with explicit CRT imports (`ucrt.lib`, `msvcrt.lib`, `vcruntime.lib`, `kernel32.lib`, `ole32.lib`).
- `gc.dll` auto-copied next to the output binary by `tyra build`; Windows DLL loader resolves it without PATH changes.
- `ilammy/msvc-dev-cmd@v1` added to CI to initialise `LIB`/`INCLUDE`/`PATH` before linking.
- `release-gate-windows` promoted from `continue-on-error: true` to **required** (merge blocker).
- Windows corpus (`bench/static-corpus/win/`) wired into `release-gate-windows` CI: `01-hello-win.tyra` (minimal binary + DLL load) and `02-gc-alloc-win.tyra` (Boehm GC allocation).
- Distribution: `tyra-<version>-windows-x86_64.zip` with `tyra.exe` + `gc.dll` co-located.

**`strtol` → `strtoll` (LLP64 fix)**
- Replaced `strtol` with `strtoll` in emitted LLVM IR. On Windows MSVC, `long` is 32-bit (LLP64); `long long` is 64-bit on all platforms, matching Tyra's `Int` (i64).

**Language spec v0.8**
- `docs/spec/{ja,en}/language-spec.md` §11: added `LinkedMap` / `LinkedSet` sections with API reference, complexity table, and comparison with `Map` / `Set`.

### Known Limitations

- **HM unification is conservative**: `types_compatible()` uses a per-call throw-away substitution rather than propagating it across the full checker pass. Full substitution threading is deferred to v0.9. Programs accepted by v0.7 continue to compile.
- **`LinkedMap.remove` / `LinkedSet.remove` is O(n)**: the entries array is fully rebuilt on each remove. Use `Map` / `Set` for workloads where removal is frequent.
- **Windows MSVC ABI only**: MinGW GNU ABI and Windows ARM64 are not supported. Native PDB debug symbols are deferred to v0.9 (DWARF debug info works on macOS/Linux).
- **AI-gen benchmark Run 18**: 86/100 pass (seed=18). Seed differs from Run 17 (seed=2); pass-count is not directly comparable. The primary v0.8.0 signal is **E0500 count = 0**.

### Not in This Release

- inkwell IR generation migration (writeln! → builder API) — v0.9
- rank-N polymorphism / type classes / where clauses — spec §22 non-goals
- Windows ARM64 / native PDB debug symbols — v0.9
- `SortedMap` / `SortedSet` (sort-order collections) — v0.9
- `LinkedMap` literal syntax (`{| k: v |}`) and `LinkedMap.from([...])` — v0.9
- Full substitution propagation across checker — v0.9

---

## [0.7.0] — Polymorphic Star (2026-05-27)

### Stable

**Type checker diagnostics — E0308 improvements**
- Added `help: Option<String>` field to `Diagnostic`; type mismatch errors now surface fix suggestions.
- Added secondary label "expected because of this annotation" to E0308, pointing to the declaration site of the expected type.
- E0308 heuristics: (i) T vs Option<T>, (ii) T vs Result<T,E> + `?` operator, (iii) Int ↔ Float conversion.
- Added deduplication by `(span, code)` to `Report` to prevent cascade floods.
- Added an impl-method return-type registry; some `Ty::Error` suppressions replaced with real return-type lookups.

**Additional diagnostic accuracy improvements**
- E0110 (`import` inside function body): `with_help` guides the user to move the import to the file top.
- E0211 (`?` at top level): `with_help` guides the user to wrap the call in `fn main() -> Result<Unit, E>`.
- E0213 (new): dedicated error code for the case where `fn main` and top-level statements coexist; previously an internal BUG panic.
- E0204 (unknown string method): errors are now pushed to `lower_errors` in MIR lowering and propagated to the driver's `Report`; unknown string methods are now `compile_fail` (previously silent with exit code 0).
- Instance methods on `List<T>` and `Option<T>` (`.get`, `.len`, `.ok_or`) are now resolved correctly by the type checker; unknown methods hard-error as E0204, eliminating E0500 LLVM crashes caused by `Ty::Error` cascades.
- **AI-gen benchmark final result (Run 17, 2026-05-28)**: 98/100 pass (98.0%) — up +7 pt from Run 16 (91%, before E0204 hard-error). Remaining 2 failures: 1 codegen edge case (E0500), 1 AI-generated syntax error.

**Persistent Collections (HAMT)**
- `Map<K,V>` and `Set<T>` reimplemented using HAMT (Hash Array Mapped Trie) as true persistent data structures.
- `m.insert(k, v) -> Map<K,V>` — returns a new Map without modifying the original.
- `m.remove(k) -> Map<K,V>` — returns a new Map without modifying the original.
- `s.insert(v) -> Set<T>` / `s.remove(v) -> Set<T>` — likewise persistent.
- Structural sharing (path-copy) keeps insert/remove at O(log₃₂ n) ≈ O(1) node copies.

**Map/Set iteration**
- `for k, v in m { ... }` — iterate over keys and values of a Map.
- `for v in s { ... }` — iterate over elements of a Set.
- E0313 "for loop binding count mismatch": reports a mismatch between the number of bindings and the iterable type.

### Experimental

**inkwell dependency (tyra-codegen-llvm)**
- Added `inkwell 0.9` as a dependency of `tyra-codegen-llvm`.
- `build.rs` auto-detects the installed LLVM version (19/20/21/22).
- Updated CI matrix to match each OS's available LLVM version.

### Known Limitations

- **E0308 heuristic (iv) not implemented**: ADT variant vs parent type suggestions deferred to v0.8+ because `Ty::Named` cannot distinguish variants.
- **inkwell IR migration incomplete**: `writeln!`-based IR generation unchanged; DWARF `DIBuilder` migration is incompatible with text IR and deferred to v0.8+.
- **Iteration order not guaranteed**: `for k, v in m` / `for v in s` order is HAMT DFS (hash order), not insertion order.
- **`Ty::Var` permissiveness unresolved**: full unification map for type variables deferred to v0.8+.

### Not in This Release

- Hindley-Milner type inference (Ty::Var substitution map)
- ADT variant type-suggestion heuristic (iv)
- Full LLVM IR generation via inkwell (writeln! → builder API)
- `LinkedMap` / `LinkedSet` (insertion-order-preserving collections)
- Custom linker (clang retained as linker driver)

---

## [0.6.0] - 2026-05-25

### Stable

**`time` and `log` stdlib modules (ADR-0014 Phase 2a)**
- `import time`: `now_unix() -> Int`, `monotonic_millis() -> Int`
- `import log`: `info(_ msg: String) -> Unit`, `warn`, `error` (writes to stderr)

**Generic `Map<K,V>` — full generalization (ADR-0015)**
- `Map<K,V>` now supports arbitrary `K: Eq + Hash` and `V`; hardcoded `Map<String,Int>` removed
- Empty-literal `{}` infers `K`/`V` from context (bidirectional type propagation); bare `{}` without expected type is a type error with a clear diagnostic
- Runtime: boxed erased-value ABI + compiler-emitted `eq`/`hash` function pointers for arbitrary key types (prims, `value` structs, ADTs)
- `Float` and `mut`-field types correctly rejected as keys (`Hash` ability not satisfied)

**Generic `Set<T>` — new collection (ADR-0015)**
- `import set`: `set.new() -> Set<T>`; method API: `s.insert(x) -> Set<T>`, `s.contains(x) -> Bool`, `s.len() -> Int`
- Requires `T: Eq + Hash`; same boxed runtime ABI as `Map<K,V>`
- `Float`/`mut`-field types rejected at compile time

**Panic expectation in `tyra test` (ADR-0012)**
- Tests named `test_panics_*` or annotated `test "name" panics` are expected to call `panic()`
- Intentional panics identified by `exit(101)` + stderr sentinel `__TYRA_PANIC__`; OOB = `exit(102)`; OOM/segfault stay as `None` (no false-pass)
- Pass = `exit(101)` + sentinel; normal return = fail; OOB/killed = fail

**`test "name"` language syntax (ADR-0013)**
- New item syntax: `test "<name>" [panics] <body> end`
- `test` and `panics` are true contextual keywords (lexer unchanged; no backwards incompatibility)
- Body lowers to a hidden `Result<Unit, String>` function; `end` inserts implicit `Ok(())`; `?` for early `Err` return
- Test discovery handles `test "name"` blocks alongside `test_*` functions; TAP/JUnit output shows the string name

**Coverage reporting — `tyra test --coverage` (ADR-0014)**
- Reports line and function coverage; branch coverage is explicitly out of scope
- Counters are `(file, line)` keyed; each basic-block entry that introduces a new source line increments the counter — no false-uncovered from multi-BB lines
- Per-test subprocesses write counters to `$TYRA_COV_DIR/<pid>.covraw` via an `atexit` handler; parent merges all files after test run
- Normal exits and panics/OOB aborts flush counters (atexit runs); `SIGKILL` timeout is best-effort (atexit may not run)

**DAP debugger — DWARF + lldb-dap + VS Code (ADR-0014 Phase 4)**
- Non-release builds (`tyra run`, `tyra build` without `--release`) emit full DWARF debug info in the generated LLVM IR: `DICompileUnit`, `DIFile`, `DISubprogram`, `DILocation` per instruction, `DILocalVariable` + `llvm.dbg.declare` for locals and parameters
- `tyra build --release` omits DWARF
- VS Code extension: `breakpoints` and `debuggers` contributions added; `TyraDapDescriptorFactory` discovers `lldb-dap` from `LLDB_DAP_PATH` env or Xcode/Homebrew LLVM candidates
- Line breakpoints, step, and local variable display work via `lldb-dap`

**Span threading through MIR (ADR-0014 Phase 1)**
- `Instruction` now carries `SourceLoc { file_id, line, col }` throughout MIR lowering
- Panic messages include the source line of the `panic()` call
- `Function.local_metas` populated with params and `mut`-binding alloca slots for DWARF locals

### Known Limitations

- DWARF locals accurate only at `-O0` (debug builds); release builds have no debug info
- Complex types (closures, GC-boxed recursive ADTs) show simplified DWARF representations
- `tyra test --coverage` with `--timeout` and `SIGKILL`: last increments in a killed test may not be visible (best-effort)
- `Set<T>` constructor is `set.new()` (no set-literal syntax to avoid ambiguity with map `{}`)
- `Map<K,V>` is immutable — no `insert`, `remove`, or iteration; all entries must be specified in a literal
- `Set<T>` has no `remove` or iteration; grow a set via chained `s.insert(x)` calls (each returns a new `Set<T>`)

### Not in This Release

- `tyra publish` / package registry
- Branch coverage
- inkwell migration (deferred — separate release scope)
- Type-checker ergonomics / E0308 diagnostic improvements

---

## [0.5.0] - 2026-05-23

### Stable

**Cross-OS CI matrix + static output binary**
- `release-gate.yml` now runs build+test+static-corpus+smoke on all three required OSes per PR: Linux glibc (ubuntu-latest), macOS arm64 (macos-15), and Alpine musl — macOS regressions are now caught before release
- `tyra build --static`: links the compiled program statically on musl (`-static`); produces a fully self-contained binary with no shared-lib deps
- CI verifies the Alpine musl artifact is statically linked (`file` check); musl static artifact added to GitHub Releases
- Windows tracking job (non-blocking allow-failure) added to surface toolchain drift
- Platform matrix: Linux glibc (dynamic), Linux musl (static), macOS arm64 (dynamic), Windows (unguaranteed / tracking)

**`string.replace` and `string.join`**
- `string.replace(_ s: String, _ from: String, _ to: String) -> String` — replaces all occurrences of `from` with `to`
- `string.join(_ parts: List<String>, _ sep: String) -> String` — joins a `List<String>` with a separator

**Per-test process isolation in `tyra test`**
- Each `test_*` function now runs in its own subprocess (compile-once, exec-per-test)
- A panic or abort in one test no longer voids sibling test results
- TAP output format unchanged; timeout (`--timeout`) applied per test
- Groundwork for `assert.panics` (deferred pending panic-semantics ADR)

**Correctness and diagnostic fixes**
- `tyra test`: `collect_test_files` now returns results in stable lexicographic path order (was filesystem order — non-deterministic across OSes and filesystems)
- `tyra test`: compile-error synthetic TAP plan corrected to `1..1` (was `1..n`; TAP consumers saw a plan/actual mismatch)
- `tyra build --static`: guard now queries `clang -print-target-triple` for "musl" instead of a compile-time `cfg!` check; error message includes the detected triple for easier diagnosis
- `tyra test --format junit`: compile-error `<failure>` element now carries the compiler diagnostic text (was empty)
- `tyra test --list`: stable output order (lexicographic file order, source-declaration function order within a file) now formally documented
- musl release artifact now includes a pre-built `examples/hello` static binary for quick verification without a full build

### Known Limitations

- `assert.panics` not yet implemented (deferred — needs panic-semantics ADR to define a distinguishable signal vs segfault/timeout)
- `tyra build --static` only reliable on musl (glibc static linking is unsupported — breaks `getaddrinfo`)
- Windows native build untested (WSL2 recommended); toolchain tracking CI job only

### Not in This Release

- `tyra publish` / package registry
- `Set<T>`, generic `Map<K,V>`, `time`/`log`/`float` stdlib
- `test "name"` language syntax
- Coverage reporting

---

## [0.4.0] - 2026-05-22

### Stable

**Lambda C ABI / closures (ADR 0011)**
- First-class lambda expressions: `fn(_ x: Int) -> Int x * 2 end`
- Closure ABI: `{ fn_ptr, env_ptr }` fat pointer; environment heap-allocated via Boehm GC
- Capture semantics: `value` → copy, `data` → reference (spec §9.4)
- `E0402` compiler error for illegal mutation of captured variables inside lambdas

**Generic `List<T>` + higher-order functions**
- `list.map`, `list.filter`, `list.fold` accept `fn(T)->U` closures
- `List<String>` fully supported alongside `List<Int>`
- `stdlib/list.tyra` updated; `__list_*` intrinsics extended

**Generic `assert.eq` / `assert.ne`**
- `assert.eq(a, b)` and `assert.ne(a, b)` overloaded for `Int`, `String`, `Bool`
- Type-checked dispatch; existing typed helpers (`assert.eq_str` etc.) retained for backward compatibility

**`tyra bench <dir>`** (spec §18.8)
- Discovers `*_bench.tyra` files in a directory and runs each, reporting wall-clock time
- `--json` for machine-readable output; `--quiet` for silent runs

**`tyra test --timeout` and parallel execution**
- `--timeout <secs>`: per-test-file wall-clock limit; timed-out tests counted as failures in TAP and JUnit
- `--jobs N`: parallel test execution (default: 1); output order is deterministic regardless of completion order
- JUnit `--format junit` now correctly reports compile/infra failures even when no test records are emitted
- Pipe-buffer deadlock prevention: stdout and stderr drained on background threads

**`Tyra.lock` + floating branch constraints + transitive dependency resolution**
- `tyra mod sync` resolves all direct + transitive dependencies and writes `Tyra.lock`
- `branch = "..."` floating constraint in `Tyra.toml`; resolved to exact SHA via `git ls-remote`; `rev` and `branch` are mutually exclusive
- `Tyra.lock` records each package: `name`, `source`, `rev`, `branch` (optional), `pkg_version` (informational); format version = 1
- `tyra mod sync --locked`: CI mode — validates manifest against existing lockfile without network access
  - Detects source, rev, branch-name, constraint-type (rev↔branch), dep-alias, and transitive path dep changes
  - Resolver keyed by canonical source (URL for git, abs path for path deps) — prevents cross-subgraph alias collisions
  - Path dep sources normalised relative to project root — correct across nested manifests at any depth
- `tyra mod show [--json]` displays resolved rev and branch for floating-constraint deps

**Resolver correctness (ADR 0009/0010 enforcement)**
- `run_sync` now calls `validate_dep_root` for path dependencies on first insert — catches `package.name ≠ dep_key`, missing `src/<name>.tyra`, and bin-package violations that previously passed silently
- Same-source-different-alias: a canonical source already in the resolved set under a different dep key now returns `NameMismatch` (path and git, both branches)
- `E0220 DepNameCollision`: two unrelated packages sharing the same `package.name` from different sources are rejected during resolution instead of producing a broken lockfile

### Known Limitations

- Registry (`tyra publish`, full registry-backed resolver) not yet available → v0.5+
- Windows native build untested (WSL2 recommended)
- `assert.panics` not yet implemented (requires per-test process isolation)

### Not in This Release

- Full registry-backed SemVer resolver, `tyra publish` → v0.5+
- `test "name"` language syntax → separate ADR required
- Pre-built binaries (Homebrew, apt) → later

---

## [0.3.0] - 2026-05-19

### Stable

**Project lifecycle — scaffolding**
- `Tyra.toml` manifest — `[package]` (name, version, edition) and `[dependencies]` (path / git+rev)
- `tyra new <name> [--lib] [--vcs none]` — scaffold a bin or lib project (`src/<name>.tyra`, `.gitignore`, `README.md`)
- `tyra mod init [--name <name>]` — create `Tyra.toml` for an existing directory

**Project lifecycle — dependency management**
- `tyra mod add <name> --path <path>` / `--git <url> --rev <rev>` — append a dependency entry
- `tyra mod update <name> --path <path>` / `--git <url> --rev <rev>` — update an existing entry in-place
- `tyra mod remove <name>` — delete a dependency entry
- `tyra mod show <name> [--json]` — print details of one dependency (source, version, cache path, synced status)
- `tyra mod tree [--json]` — render the dependency tree; `--json` emits structured JSON (cycle detection, diamond DAG safe)
- `tyra mod sync [--check] [--json] [--quiet]` — clone git deps; `--check` validates without mutating; `--json` / `--quiet` for CI use
- `tyra mod clean` — remove `~/.tyra/cache/`

**Project lifecycle — zero-arg project commands**
- `tyra run [--release]` — inside a project dir, discovers entry point from `Tyra.toml`; `--release` enables `-O2`
- `tyra build [--release] [-o <out>]` — same discovery; output binary placed at project root; `-o` overrides destination
- `tyra check` — same discovery; type-checks the project entry point

**Import resolution (ADR 0010)**
- Three-layer uniqueness rule: local `src/` → `[dependencies]` → stdlib
- `E0217` on ambiguous import (two layers provide the same module name); no silent shadowing
- `E0218` for bin package dependencies and dep key / `package.name` mismatches

**Dependency invariants (ADR 0009)**
- Bin packages cannot be imported (`E0218`)
- Dependency key must equal `package.name` in the target manifest (no aliasing)
- Root module `src/<name>.tyra` must exist at `tyra mod sync` time
- All invariant checks apply to both fresh clones and stale/manually-populated caches

**Test runner improvements**
- `tyra test --filter <pattern>` — substring match on `test_*` function names to run a subset
- `tyra test --list [--filter <pattern>]` — list matched test functions without executing
- `tyra test --format junit` — emit JUnit-compatible XML (`<testsuites>` / `<testsuite>` / `<testcase>`)
  - Infrastructure failures (compile errors) produce a synthetic single-test suite so CI always sees a concrete failure
  - Each `<testsuite>` carries a `time=` attribute sourced from the per-file wall-clock elapsed
- TAP output now includes a `# time: <s>s` comment at the end of each file's run

**Formatter improvements**
- `tyra fmt [--check] [--stdin] <file|dir>` — `--stdin` reads from stdin, writes formatted source to stdout; composable with editors and pipes
- Line-length wrapping (100-col limit) — long function signatures wrap one-param-per-line; idempotent

**AI benchmark**
- `tyra bench ai-gen [options]` — thin wrapper over `bench/ai-gen/harness.py`; all harness flags forwarded verbatim

**Documentation**
- `docs/getting-started/09-project-lifecycle.md` — full lifecycle guide (tyra new → mod add → mod sync → build)
- `docs/getting-started/08-testing.md` — expanded: `--filter`, `--list`, JUnit XML, timing
- `docs/design/0009-project-manifest.md` and `docs/design/0010-dependency-resolution.md` — ADR rationale

### Known Limitations

- `Tyra.lock` and floating version constraints not yet supported (path and git-rev pin only); `Tyra.lock` + minimal solver planned for v0.4.0
- Registry (`tyra publish`, crates.io equivalent) not yet available; planned for v0.5+
- Windows native build untested (WSL2 recommended)

### Not in This Release

- Lambda C ABI, generic `List<T>`, `map`/`filter`/`fold` → v0.4.0
- `Tyra.lock` + floating version constraints + transitive dependency resolution (minimal solver) → v0.4.0
- `tyra test --timeout`, parallel test execution → v0.4.0
- Full registry-backed SemVer resolver, `tyra publish` → v0.5+
- Pre-built binaries (Homebrew, apt) → separate release

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
  - E0216: `*_test.tyra` files must not contain `fn main` or top-level executable statements

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
- `tyra fmt` line-length enforcement and expression wrapping — deferred to v0.2.x
- `tyra test --filter <pattern>` — deferred to v0.2.x
- `assert.panics` — requires per-test process isolation; deferred
- Generic `assert.eq<T>` — requires trait bound support; deferred

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
