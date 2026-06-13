# ADR 0029: Runtime semantics of `fn main() -> Result<Unit, E>` returning `Err` (B2)

- **Status**: Accepted
- **Date**: 2026-06-12
- **Spec sections affected**: entry-point section (§ around `fn main`, cf. ADR-0006), async type rules (`main` forms)

## Context

The spec permits `fn main() -> Result<Unit, E>` and
`async fn main() -> Result<Unit, E>` (type rules; ADR-0006 entry-point
styles), but never defines what happens when `main` actually returns `Err`.

The implementation today does the worst possible thing: **the program exits
with status 0 and prints nothing**. Confirmed on HEAD (2026-06-12) with a
2-line repro; first observed via ai-gen case 064-count-down, where a program
whose argument parsing failed reported success to the shell. A language whose
core claim is "errors are visible values" silently discards the final,
outermost error.

Maintainer decision (2026-06-12): report to stderr and exit non-zero, in
v0.11.0, with the behaviour written into the spec.

## Decision

When `main` (sync or async) returns `Err(e)`:

1. The runtime writes one line to **stderr**:

   ```text
   error: <Debug representation of e>
   ```

   using the same Debug rendering that `print` / `eprintln` apply to a
   Debug-able value. The anchor is deliberately **not** string interpolation:
   interpolability and the Debug ability are different boundaries in Tyra
   (E0314 rejects non-displayable interpolation), and tying this spec to
   interpolation would misread as "only interpolable error types allowed".
2. The process exits with **status 1**.
3. `defer` blocks run before the error is reported (normal scope exit —
   returning `Err` is a regular return, not a panic).
4. `Ok(())` continues to exit 0. Behaviour of `fn main() -> Unit` and
   top-level style (implicit `fn main() -> Unit`, ADR-0006) is unchanged.

### Exit-code map (spec table)

| Outcome | Exit status |
|---|---|
| `main` returns `Unit` / `Ok(())` | 0 |
| `main` returns `Err(e)` | 1 (after printing `error: …` to stderr) |
| `panic(...)` | 101 (unchanged; `tyra run` reports it as E0501) |
| `core.sys.exit(n)` | n (explicit, unchanged) |

### Constraint on `E`

`E` must satisfy the **Debug ability constraint** so the runtime can render
it. In practice every current type auto-derives Debug (§8.6); should a
non-Debug `E` become expressible, the checker rejects the `main` signature
with a dedicated diagnostic rather than failing at print time.

### `tyra run` behaviour

`tyra run` propagates the child's exit status. Status 1 from an Err return is
a **normal program outcome** — `tyra run` must not wrap it in E0501 (which is
reserved for abnormal termination such as panic's 101). Implementation must
verify E0501 triggers on its current condition only.

## Implementation scope note (2026-06-12, maintainer decision)

Implementation found that the runtime has **no Debug rendering for
non-displayable types**: `print(adt_value)` compiled and crashed at runtime
(now rejected at compile time by E0319, which applies the E0314 displayable
whitelist to the print family). Until ADT Debug rendering exists, the Err
report renders as:

- `E` displayable (Int / Float / Bool / String / Option of these / tuples of
  these): `error: <value>` — full payload.
- locally-defined non-generic ADT: `error: <variant name>` — e.g. `error: Timeout`.
  Field values elided; extends to full Debug rendering in a future release (v0.12 candidate).
- any other `E`: `error: main returned Err(<type name>)` — type name only.

Exit status 1 applies in all cases. The spec text for 0.11 documents this
three-tier rendering explicitly.

### As landed (2026-06-12)

- **Mechanism**: the driver desugars `fn main() -> Result<Unit, E>` after
  type checking — the original main is renamed `__tyra_main_inner` and a
  synthesized wrapper (parsed from generated source) matches the result,
  reports via `eprintln`, and calls the `sys__exit` codegen sentinel
  (registered unconditionally in MIR lowering). Sync and async main both
  covered; `defer` blocks run at inner-scope exit, before the report.
- **Pre-existing bug found and fixed**: `eprint`/`eprintln` wrote to
  **stdout** — they lowered to the same printf/puts path as `print`. MIR
  now normalizes the eprint argument to a single String (interpolation
  formatting reused for scalars) and codegen routes it to new runtime
  stderr writers `tyra_eprint_str` / `tyra_eprintln_str`.
- **`tyra run` exit codes**: `run`/`run_release` now return
  `ProgramRunResult { compile, program_exit }`; the CLI propagates the
  child's exit code. E0501 fires only for status 101 (panic), signal
  kills, and spawn failures — exactly the table above.
- Verified by 5 CLI integration tests (`err_main_*`, `ok_main_*`,
  `run_propagates_*`, `panic_is_still_e0501`) plus the full corpus.

### Review fixes (2026-06-12, second pass)

- **Free `main` references follow the rename**: `rename_free_fn_refs`
  rewrites every free reference to `main` across all fn bodies, impl
  methods, and top-level statements (self-recursion included), respecting
  local shadows (let/mut/tuple/for/lambda params/pattern bindings).
- **Displayability uses the checker's own boundary**:
  `tyra_types::Ty::from_type_expr(E).is_interp_displayable()` — no
  syntactic re-approximation. Empirically on HEAD this boundary excludes
  `Option<Bool>` (E0314 rejects it in user code too) and type aliases
  (the checker performs no alias expansion anywhere — alias-typed scalar
  bindings already fail E0308). Both therefore take the type-name
  fallback, consistently with what user-written interpolation would do.
- **Pre-existing payload bugs surfaced while testing** (independent of
  this ADR — the inner `Err(...)` construction itself miscompiles):
  `Result<Unit, MyErr>` with `type MyErr = String` emits mismatched
  struct names (`Result__Unit__MyErr` vs `Result__Unit__String`), and
  `Result<Unit, (Int, String)>` trips a `Tuple2__`/`Tuple__` struct-name
  inconsistency. Recorded as follow-ups alongside the known alias gaps.

## Consequences

- Shell scripts, CI, and agents finally observe failure: `tyra run prog.ty
  || handle` works as every Unix user expects.
- **Breaking** for any program that (knowingly or not) relied on Err-main
  exiting 0 — almost certainly latent bugs; pre-1.0 policy applies.
- The Debug rendering of `e` becomes user-visible output; error type authors
  get readable top-level failures for free.
- Spec gains an explicit exit-code table — also useful for the Playground
  and the ai-gen harness, which both inspect exit codes.

## Alternatives considered

| Option | Rejected because |
|---|---|
| Exit 1, print nothing | Silent failure with a hint; debugging requires wrapping main by hand — boilerplate the `?` design exists to remove |
| Exit 101 (same as panic) | Err-return is a controlled outcome, not abnormal termination; conflating them destroys exit-code information |
| Print to stdout | stdout is program output; error reports belong on stderr |
| Require user-written `match` in main (forbid Result main) | Spec already permits Result main; removing it would break the `?`-in-main ergonomics that ADR-0006 deliberately enables |
| Stringable-based rendering | Not all error types implement Stringable (a trait, opt-in); Debug is an ability and auto-derived (§8.6) — "ability constraint" terminology per project convention |
