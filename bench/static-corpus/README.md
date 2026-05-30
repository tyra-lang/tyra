# Static Corpus

Hand-written, compiler-team-verified Tyra programs pinned as a regression
baseline.  Every file here must compile (or fail with a known expected error)
on the current `main` branch.

## Purpose

* **Regression detection** — catch regressions instantly when compiler internals
  change, independent of AI-generated output quality.
* **Reference programs** — authoritative examples of correct Tyra code; useful
  for AI prompt engineering and onboarding.
* **Spec coverage** — each file cites the spec sections (§) it exercises.

## Positive corpus

Files in the top-level directory are expected to compile without errors.

| File | What it tests |
|------|---------------|
| `01-hello.tyra` | Top-level executable statement, `print` |
| `02-fibonacci.tyra` | Recursive fn, `match` on Int literals, forward reference |
| `03-option-result.tyra` | `Option`/`Result`, `?`, `ok_or()`, ADT patterns |
| `04-http-handler.tyra` | `http.server`, `import`, `data` type, request/response |
| `05-json-parsing.tyra` | `json` stdlib, `Into` impl, nested ADT patterns |
| `06-cli-args.tyra` | `core.sys.args`, `List.get()`, turbofish `parse::<T>` |
| `07-state-machine.tyra` | `value` type, `copy()`, ADT qualified construction |
| `08-async-tasks.tyra` | `async fn`, `spawn`, `Task<T>`, `.await?`, `join_all` |
| `09-error-handling.tyra` | `defer`, `panic`, compound `and`/`or`, `Into` chain |
| `10-data-modeling.tyra` | `value`/`data` distinction, `Stringable`, `Eq`/`Ord` |
| `11-break-loop.tyra` | `break` inside `while` and `for` (§10.4, §10.5) |
| `25-nested-match-map-get.tyra` | Nested `match` on `io.read_line()` + `Map.get()` — E0500 regression guard (§10.3, §17.3.6) |
| `26-linked-map-order.tyra` | `LinkedMap` insert/iteration compiles; runtime order verified in `linked_map_order_test.tyra` (§11, ADR-0019, v0.8.0) |
| `27-hm-inference-empty-map.tyra` | Empty map literal `{}` resolves via HM unification from type annotation without `Ty::Error` (ADR-0020, v0.8.0) |
| `28-linked-set.tyra` | `LinkedSet` insert/contains/remove/len/for-loop compiles; runtime correctness in `linked_set_test.tyra` (§11, ADR-0019, v0.8.0) |

## Negative corpus (`bad/`)

Files in `bad/` are expected to **fail** with a specific error code.
File names follow the pattern `Exxxx-<slug>.tyra`; `check.sh` extracts the
expected code from the filename and verifies that `tyra check` exits non-zero
and that stderr contains `error[Exxxx]`.

| File | Expected error |
|------|----------------|
| `bad/E0104-unexpected-token.tyra` | E0104 — parser: dangling operator |
| `bad/E0200-undefined-name.tyra` | E0200 — resolve: undefined identifier |
| `bad/E0206-assign-to-immutable.tyra` | E0206 — type: assign to immutable `let` binding |
| `bad/E0214-break-outside-loop.tyra` | E0214 — type: `break` outside of a loop |
| `bad/E0301-arity-mismatch.tyra` | E0301 — type: function call with wrong argument count |
| `bad/E0302-question-mark-on-non-result.tyra` | E0302 — type: `?` applied to non-Option/Result |
| `bad/E0305-arithmetic-type-mismatch.tyra` | E0305 — type: arithmetic type mismatch |
| `bad/E0309-return-type-mismatch.tyra` | E0309 — type: fn return type mismatch |
| `bad/E0400-non-exhaustive-match.tyra` | E0400 — type: non-exhaustive match |
| `bad/E0308-adt-variant.tyra` | E0308 — type: ADT variant used where parent type expected (heuristic iv, v0.8.0) |

### Documentation-only fixtures (not CI-checked by `check.sh`)

Some files in `bad/` are **documentation fixtures** rather than automated regression guards. `check.sh` only verifies files whose filename starts with `Exxxx-` (an error code prefix) and exits non-zero on unexpected output. The following file does not follow that pattern and is intentionally excluded from automated checking:

| File | Purpose |
|------|---------|
| `bad/E9001_no_type_error_reaches_codegen.tyra` | Documents the expected E0308 diagnostic for a Map value-type mismatch. Verifies that E9001 (ICE: unresolved type reached codegen) does **not** appear — i.e., the compiler diagnoses the error before codegen rather than crashing. This is a property that can only be confirmed by running `tyra check` manually, not by pattern-matching a fixed error code in `check.sh`. The E9001 guard itself is unit-tested in `tyra-codegen-llvm/src/lib.rs`. |

## Running the corpus

```bash
# Compile-check all files (excluding examples that require network / runtime)
for f in bench/static-corpus/*.tyra; do
  tyra check "$f" && echo "OK  $f" || echo "FAIL $f"
done
```

Alternatively, the CI script at `bench/static-corpus/check.sh` runs both the
positive and negative corpus and exits non-zero on any failure.

```bash
bash bench/static-corpus/check.sh ./target/debug/tyra
```

The GitHub Actions workflow at `.github/workflows/static-corpus.yml` runs this
check automatically on every push to `main` and every pull request targeting
`main`.  A failing corpus file turns the CI job red before it can land.

## Spec coverage

`bench/static-corpus/coverage.sh` cross-references every `# SPEC_REF: §X.Y`
annotation against the section headings in `docs/spec/ja/language-spec.md`
and prints:

* which spec sections are covered by at least one corpus file,
* which are uncovered, and
* which references point to non-existent section headings.

```bash
bash bench/static-corpus/coverage.sh
```

The script is informational (exits 0); it is not run in CI.

## Adding programs

### Positive corpus

1. Write the program; verify it compiles on the current compiler.
2. Add the spec section references as `# SPEC_REF: §...` comments.
3. Add a row to the positive corpus table above.
4. Do NOT add programs that are expected to fail at runtime (runtime-only
   failures belong in `bench/ai-gen/`).

### Windows corpus (`win/`)

Files in `win/` are smoke-test programs specifically for the Windows MSVC ABI target (ADR-0021, v0.8.0). They are **not** checked by the default `check.sh` (which targets the host platform). They are run by the `release-gate-windows` CI job (`.github/workflows/release-gate.yml`, "Windows corpus" steps) and can also be compiled manually with:

```powershell
.\target\debug\tyra.exe build bench\static-corpus\win\01-hello-win.tyra
.\target\debug\hello-win.exe   # gc.dll must be in the same directory
```

| File | What it tests |
|------|---------------|
| `win/01-hello-win.tyra` | Minimal binary runs; gc.dll loads from same-dir (ADR-0021) |
| `win/02-gc-alloc-win.tyra` | GC allocation via `List<Int>` — Boehm GC initialises correctly |

### Negative corpus

1. Write the program so that exactly one `error[Exxxx]` is emitted.
2. Name the file `Exxxx-<short-slug>.tyra` inside `bad/`.
3. Add the following header comments:
   - `# expected: error[Exxxx]: <short description>` (exact compiler message)
   - `# SPEC_REF: §... — <description>` (spec section being exercised)
4. Verify with `tyra check bad/Exxxx-<slug>.tyra` that stderr contains `error[Exxxx]` and exit is non-zero.
5. Add a row to the negative corpus table above.
