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
- any other `E`: `error: main returned Err(<type name>)` — type name only,
  payload elided. Extends to full Debug rendering when print/eprintln gain
  it (follow-up, v0.12 candidate).

Exit status 1 applies in all cases. The spec text for 0.11 documents this
two-tier rendering explicitly.

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
