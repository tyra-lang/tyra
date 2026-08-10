# ADR 0030: Affine task-handle tracking (v1, conservative)

- **Status**: Accepted
- **Date**: 2026-08-10
- **Spec sections affected**: §14 (new §14.5)

## Context

The reference runtime (`runtime/src/lib.rs`) treats a `Task` handle as
consumed exactly once. `tyra_task_await` does `Arc::from_raw(task)`,
reclaiming the awaiter's strong reference; a second call on the same pointer
is a double-free — undefined behavior, not a panic. `tyra_task_join_all`
awaits (consumes) every element of its list in order. `tyra_task_select`
does **not** consume its source handles — it clones an `Arc` per source
internally so the dispatcher and the caller each hold an independent
reference — but its own returned handle is a fresh, ordinary once-only
handle.

Until now this contract existed only in a doc comment
(`runtime/src/lib.rs:381-383`, pre-this-change wording): *"The Tyra compiler
is responsible for ensuring each handle is awaited exactly once."* No such
enforcement existed. Nothing prevented safe-looking Tyra source from
double-freeing:

```tyra
let t = spawn fetch(url)
let a = t.await
let b = t.await   # UB: second Arc::from_raw on a freed pointer
```

This is reachable from ordinary, non-`unsafe` Tyra code — a language whose
core promise is memory safety without a borrow checker (§15.2) cannot leave
this open indefinitely.

## Decision

Add a **standalone, purely syntactic AST pass** (`tyra-types/src/task_affine.rs`)
that tracks `let`-bound task handles per function-like body (fn bodies, impl
methods, and the implicit top-level `main`, ADR-0006 Rule 5) and flags the
statically-detectable misuses. It does not participate in `infer_expr` and
needs no `TypeEnv` — `spawn`, `.await` on a bare identifier, and
`tasks.join_all([...])` / `tasks.select([...])` are all syntactically
identifiable without type information.

**Tracked bindings** (v1, deliberately narrow): `let x = spawn ...` and
`let x = tasks.select([...])`. Nothing else — in particular
`tasks.join_all(...)`'s own result is not tracked (its element consumption
is what matters, not its own handle).

**Per-binding lattice**, scoped to a stack mirroring lexical scopes:

```
Live ──consume──▶ Consumed
 │                    │
 └──escape──▶ Untracked (absorbing; never diagnosed again)
```

- `x.await` on a tracked ident: `Live → Consumed`. Consuming an already
  `Consumed` handle is **E0321** ("task handle is awaited more than once").
- `x` as a *direct* element of the list literal argument to
  `tasks.join_all([...])`: consumes, same as `.await` (mirrors
  `tyra_task_join_all` awaiting each element). Consuming an already-consumed
  handle here is E0321 too — including the case where the handle was
  consumed by a prior individual `.await`.
- `x` as an element of `tasks.select([...])`: **does not consume** — mirrors
  `tyra_task_select`'s Arc-clone semantics. The handle stays exactly as it
  was; it must still be awaited individually by the caller (see
  "select and its sources" below). The select call's own result, if bound
  via `let`, is a **fresh** `Live` tracked handle.
- Anything else a tracked ident is used for — passed as a function argument,
  stored in a list/tuple/map/constructor, field access, `return x`, captured
  by a lambda, aliased via `let y = x`, reassigned, or any use that is not
  one of the two forms above — **escapes** it to `Untracked` silently. This
  is the v1 boundary: once a handle leaves local, straight-line tracking, the
  compiler no longer reasons about it. The exactly-once contract still holds
  at runtime; it is just not statically checked for escaped handles.
- Loop nesting: consuming a handle whose binding-time loop depth is
  shallower than the current loop depth is **E0321** (a variant message
  calling out the loop), independent of the Live/Consumed state — a single
  textual `.await` inside a loop can execute more than once per handle at
  runtime. Bindings created inside the loop body are fresh per iteration and
  follow the normal rules. A `while` loop's *condition* is analyzed at the
  same depth as its body (it re-evaluates every iteration too), so a consume
  in the condition is subject to the same rule — unless the body
  unconditionally exits the loop (a top-level `break`/`return` with no
  `continue` reachable before it, the same scan as the suppression rule
  below), in which case the condition provably evaluates exactly once and is
  analyzed at the *enclosing* depth. This collapse applies to the condition
  only: applying it to the body as well would break the `break` suppression
  rule's depth test in nested loops.

  This is suppressed — the consume is provably single-run, not merely
  syntactically inside a loop — when a top-level `break` or `return`
  unconditionally follows the consuming statement within its own statement
  list (never descending into a nested `if`/`match`/loop/lambda body):
  - `return` suppresses at **any** loop depth: it leaves the function
    entirely, so no further iteration of any enclosing loop can run.
  - `break` suppresses **only** when the handle was bound directly outside
    the one loop being broken (`handle.loop_depth + 1 == consume's
    loop_depth`) — it unwinds exactly one loop level, so it does not prove
    anything about a handle bound further out that an enclosing loop could
    still re-run.
  - A `continue` reachable before the `break`/`return` on the same path
    defeats the suppression (forces it back to "no proof"): the loop can
    iterate again via the `continue`, so the consume is not actually
    single-run. This scan is purely syntactic — it looks inside `if`/`match`
    bodies on the same path but does not descend into a nested loop or
    lambda body, whose own `continue`/`break` target *that* loop, not this
    one.
    The scan is statement-granular and rooted at each statement's
    `if`/`match` (including the value expression of a `return`); it does not
    descend below other expression nodes. The only positions where the
    parser accepts a block-form `if`/`match` under another expression node
    are binary-operator operands (including `and`/`or`), assignment
    right-hand sides, and `let`/`mut` initializer operands — a `continue`
    hidden there is not seen, so a later `break`/`return` still suppresses
    the loop-depth E0321 and a real repeat-await goes unreported. Delimited
    positions (call argument, list element, string interpolation) cannot
    contain `continue` at all (parser error E0104). Conversely the scan is
    path-insensitive: a `continue` and a consume in mutually exclusive
    branches of the same statement defeat the suppression and report E0321
    even though the consume is single-run, and suppression never propagates
    into a nested `if`/`match` body. These are accepted v1 gaps: closing
    them requires per-path reachability analysis, explicitly out of scope;
    each has a trivial rewrite.
  - The suppression is recomputed fresh per statement list and is reset at
    the entry to every loop's own body/condition — an exit statement
    lexically *outside* a loop (e.g. a `return` written after the loop ends)
    cannot retroactively prove anything about a consume *inside* that loop's
    body or condition.
- Branch merge (`if`/`else`, `match` arms): pointwise join,
  `Consumed ⊔ Consumed = Consumed`, `Live ⊔ Live = Live`, any other
  combination (including a prior `MaybeConsumed`) joins to `MaybeConsumed`;
  a name missing from either side (escaped on that path) makes the merged
  result `Untracked` too. Consuming a `MaybeConsumed` handle is **allowed**
  and transitions it to `Consumed`; a further consume after that still
  errors. This deliberately resolves toward zero false positives rather than
  catching every real double-await across branches — see "the documented
  gap" below.
- Scope end: a handle still `Live` when its lexical scope closes is
  **E0322** ("task handle is never awaited") — a warning, not an error. The
  task still runs to completion; only the caller-side handle (and the
  ability to retrieve its result, or to `join_all` it) is lost. A `Live`
  handle that is used exactly once as a `tasks.select` source and never
  individually awaited also warns here, by design (see below).

**select and its sources.** `tyra_task_select` does not cancel the tasks
that do not win — v0.1 has no cancellation (§14.4) — they run to completion
regardless. The caller's Arc for each source is untouched by `select`
itself, so every source handle must still be individually awaited by the
caller to avoid leaking it (documented in `runtime/src/lib.rs`'s
`tyra_task_select` doc comment: "Failing to do (a) leaks one Arc per source
per select call"). The affine pass reflects this directly: `select`
elements are not consumed, so a source that is only ever passed to `select`
and never awaited individually correctly warns E0322 — the warning is the
static nudge toward the leak-free pattern, not a false positive. Separately,
passing an already-`Consumed` handle as a `select` source is **E0321**: the
runtime does `Arc::increment_strong_count` on every source at the call site
(`runtime/src/lib.rs`'s `tyra_task_select`), which is UB on a handle whose
Arc was already reclaimed by a prior `.await` — this check does not use the
loop-depth rule, since repeated `select` of a `Live` handle across loop
iterations is safe.

**The documented gap.** The branch-merge lattice is asymmetric by design:
it never reports a false E0321 for legitimate divergent-branch code (each
branch awaits once, no further use), but it can miss a real double-await
when one branch awaits and the other does not, followed by an unconditional
await after the merge:

```tyra
let t = spawn f()
if c
  t.await
end
t.await   # real UB when c is true at runtime — NOT flagged in v1
```

This is accepted for v1: the two "Live/Consumed mismatch" cases (partial
consume then reuse, vs. a `MaybeConsumed` reuse after merge) are
indistinguishable without either flow-sensitive path tracking or accepting
false positives on legitimately safe divergent code, and false positives are
the explicitly ruled-out failure mode for a first release of this check
(unlike e.g. E0400 exhaustiveness, getting this wrong blocks correct
programs from compiling at all). A missed double await here is genuine
undefined behavior with **no guaranteed failure mode**: `tyra_task_await`
does `Arc::from_raw(task)` before ever touching the inner `Mutex` — the
second call operates on an already-freed allocation, so the failure surface
is a use-after-free, not a poisoned-lock panic. There is no debug-build
safety net; this gap is accepted purely because false positives would block
otherwise-correct programs from compiling, not because the runtime fails
safely when it is missed. A future version may special-case this specific
shape (single-branch-then-merge) once the false-positive risk on other
divergent shapes is better understood.

**Diagnostics.** New codes `E0321` (error, double await / join_all-after-await
/ loop-repeat-await) and `E0322` (warning, never awaited), continuing the
checker's E03xx range (max in use was E0320).

## Consequences

- The double-await UB documented in `runtime/src/lib.rs` is now caught at
  compile time for the straight-line and simple-loop cases that make up the
  overwhelming majority of real spawn/await code (confirmed against
  `bench/static-corpus/08-async-tasks.ty`, the stdlib, and the example
  corpus — zero new diagnostics on any of them).
- `tasks.join_all` / `tasks.select` gain real checker signatures (spec §17)
  as part of the same change, replacing a `Ty::Error` escape hatch — a
  prerequisite for the affine pass to type `join_all`/`select` call shapes
  at all, and independently a correctness fix (their results were
  previously untyped, matching the pattern already fixed for other builtin
  modules in ADR-0028).
- Handles that escape local scope (stored in a struct field, returned across
  a call boundary and re-awaited elsewhere, threaded through a data
  structure) are **not** covered. The `Ty::Generic("Task", _)` type still
  reaches the runtime unchecked in those cases; the exactly-once contract is
  upheld only by the programmer, exactly as before this ADR. This is a
  known, accepted scope boundary, not an oversight — see "Alternatives
  considered" below.
- New compile-time diagnostics on previously-accepted programs are possible
  (a real double-await that used to silently miscompile now fails to
  compile) — an intentional, allowed breaking change pre-v1.0 (§3).
- The pass adds one AST walk per function-like body at check time;
  negligible compile-time cost (no type inference, no unification).

## Alternatives considered

| Option | Rejected because |
|---|---|
| Full linear/affine **types** (a real `Task<T>` ownership discipline enforced by the type system, e.g. a `move`-by-default value with no implicit reference semantics) | Correct long-term direction, but a much larger change — Tyra has no ownership/borrow system today (§15.2 explicitly rejects one), and retrofitting one just for `Task<T>` would be a special-cased, inconsistent carve-out rather than a general mechanism. Revisit if/when Tyra grows a broader resource-typing story. |
| Resolver-based tracking (fold this into `tyra-resolve` alongside existing scope/binding analysis) | The resolver's `ScopeStack` model tracks *names*, not per-name consumption state, and runs before the checker has established which calls are actually `tasks.join_all`/`select` shapes vs. ordinary calls to same-named user functions. The checker crate is a more natural home and keeps the resolver's scope unchanged. |
| Runtime refcount guard (e.g. a debug-only tombstone check in `tyra_task_await` that panics instead of double-freeing) | Turns a compile-time-catchable class of bug into a runtime-only, debug-build-only safety net — strictly weaker, and does nothing for the "AI self-correction" goal (ADR programs, `--error-format json`) of surfacing the mistake as a diagnostic before the program ever runs. Complementary at best, not a substitute; not pursued now since it doesn't reduce the need for the static check. |
| Catch every case, including cross-branch and escaped handles, accepting some false positives | Explicitly ruled out by the task brief: a false positive here blocks otherwise-correct programs from compiling, which is a worse failure mode for a first release than a missed diagnostic on a runtime bug class that is rare in practice (real double-awaits are a copy-paste-style mistake, not an idiom anyone reaches for intentionally). |
