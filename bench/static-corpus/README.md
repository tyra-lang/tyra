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

## Files

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

## Running the corpus

```bash
# Compile-check all files (excluding examples that require network / runtime)
for f in bench/static-corpus/*.tyra; do
  tyra check "$f" && echo "OK  $f" || echo "FAIL $f"
done
```

Alternatively, the CI script at `bench/static-corpus/check.sh` does the same
check and exits non-zero on any failure.

## Adding programs

1. Write the program; verify it compiles on the current compiler.
2. Add the spec section references as `# SPEC_REF: §...` comments.
3. Add a row to the table above.
4. Do NOT add programs that are expected to fail at runtime (runtime-only
   failures belong in `bench/ai-gen/`).
