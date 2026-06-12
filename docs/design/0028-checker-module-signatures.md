# ADR 0028: Type-checking imported module function calls (B1)

- **Status**: Accepted
- **Date**: 2026-06-12
- **Spec sections affected**: none (the spec already implies this; the implementation does not comply)

## Context

`tyra-types::check()` operates on a single `SourceFile`. Imports are recorded
only as a name map (`env.imported_modules`); the checker has no signatures for
the imported functions. A handful of modules get hand-written special cases
(`assert.eq`, `set.new`, `LinkedMap.*`, string-fold shapes, …), but **every
other imported call is silently typed `Ty::Error`** via the
`FieldAccess`-call fallback (`checker.rs:2469-2559`).

`Ty::Error` is the anti-cascade escape hatch: any expression containing it is
exempt from further checking (`infer_binop` returns early, `checker.rs:3011`).
The net effect, confirmed during the 2026-06-12 ai-gen triage:

- `let y: Int = string.from_byte(65)` passes `tyra check` (it is a `String`).
- `mut r = ""` … `r = r + string.from_byte(x)` passes `tyra check`, then
  crashes codegen with an inkwell ICE
  (`Found PointerValue … but expected the IntValue variant`) — ai-gen cases
  053-toggle-case and 100-pipeline.
- Entire stdlib-heavy programs are effectively type-checked only by MIR
  lowering's separate, partial typing.

This contradicts spec §8 (static typing) and the project's core promise:
errors must surface as diagnostics, not as ICEs or silent miscompiles.
Maintainer decision (2026-06-12): fix in v0.11.0; v0.10.2 was skipped.

## Decision

Thread **exported function signatures of imported modules** into the checker.

1. **Source of truth: parsed module ASTs.** The driver already locates and
   parses every imported module for MIR lowering. Before type-checking the
   entry file, the driver collects each imported module's `export fn`
   declarations into a `ModuleSignatures` map:

   ```text
   ModuleSignatures: module canonical name → (fn name → Ty::Fn(params, ret))
   ```

   No hand-written signature table — the stdlib `.ty` files are the single
   source of truth, so new stdlib functions are picked up automatically and
   the table cannot drift.

   **Type-name normalization** (what `Ty` a signature's type names become):

   - *Structural types* resolve structurally, independent of module:
     primitives (`Int`, `Float`, `Bool`, `String`, `Unit`, `Never`),
     builtin generics (`List`, `Option`, `Result`, `Map`, `Set`,
     `SortedMap`, `SortedSet`, `LinkedMap`, `LinkedSet`, `Task`), fn types,
     and tuples.
   - *Named types defined in the module itself* normalize to a
     **module-qualified nominal type**: `export fn open(_ p: String) ->
     Result<Unit, FsError>` in module `fs` registers `FsError` as
     `Ty::Named("fs.FsError")`. Cross-module nominal equality is by
     (defining module, type name) — consistent with Tyra's nominal typing
     (§8) and with how the entry file already names these types
     (`fs.FsError`).
   - *Types the module itself imported* resolve through **that module's own
     import map** to their defining module's qualified name before
     registration (one-hop chains collapse to the origin).
   - *Unresolvable names* (parse succeeded but the type's defining module
     cannot be determined) register the function with `Ty::Error` **in that
     position only**, preserving today's behaviour for that parameter while
     the rest of the signature still checks. A debug-level note records the
     gap so it surfaces in development rather than silently persisting.
   - *Type aliases* (§8.5, `type UserId = Int`) are **expanded to their
     target type at collection time**, recursively, before normalization —
     the registered signature contains no alias names. Alias expansion uses
     the defining module's own scope (an alias of an imported type expands
     through that module's import map like any other name). ADTs declared
     with `type ... = | ...` are not aliases; they normalize as
     module-qualified nominal types per the rule above.
2. **API change**: `tyra_types::check(file, report)` gains a parameter
   (`check(file, &module_sigs, report)`); the driver builds `module_sigs`
   from its existing module-resolution pass. User modules (project-local
   imports) flow through the same mechanism — this is signature threading,
   not stdlib-only hardcoding.
3. **Checker lookup order** in the `FieldAccess`-call path:
   1. existing special cases (semantic checks richer than signatures:
      `assert.eq` Float rejection, `set.new` inference, …) — kept;
   2. `ModuleSignatures` lookup → arity check (E0301) + argument
      `check_type_match` + declared return type;
   3. unknown function on a known imported module → **new diagnostic E0318**
      ("module `string` has no exported function `fro_byte`"), replacing
      today's silent `Ty::Error` (E0315–E0317 were already taken);
   4. unknown receiver → existing E0204 fallback.
4. **Generics limitation**: module functions whose signatures use type
   parameters beyond the checker's current stdlib-generics support are
   registered with their declared shape; where unification is not yet
   possible the checker falls back to the declared return type without
   argument unification (still strictly better than `Ty::Error`).
5. **Rollout safety**: this makes previously-unchecked code checkable, so
   some existing programs will newly fail to compile — that is the point,
   but it must be controlled. Gate: full `bench/static-corpus/` run, stdlib
   self-check, examples/, and an ai-gen re-sweep before release. Newly
   failing corpus entries are triaged: checker false positive → fix checker;
   true positive → the program was silently wrong all along.

## Implementation note (2026-06-12, as landed)

Investigation during implementation found that `resolve_imports`
(tyra-driver) already **merges** each imported module's exported items into
the entry AST — functions under mangled names (`{local}__{fn}`, e.g.
`string__from_byte`), type definitions and impls as-is — before
`tyra_types::check()` runs. `collect_top_level_types` therefore already
registers every module function's full signature, with alias expansion and
nominal types handled by the existing single-file machinery.

The landed fix is accordingly simpler than Decision 1–2 anticipated:

- **No `ModuleSignatures` map and no `check()` API change.** The checker's
  `FieldAccess`-call path bridges `m.f(args)` to `env.lookup("m__f")`
  (guarded by `imported_modules` membership and the absence of a value
  binding named `m`).
- Decision 1's type-name normalization is subsumed by the merge model:
  merged items are checked in the entry file's namespace, identically to
  same-file code.
- Decision 4 (generics limitation) landed in two layers (refined per review,
  2026-06-12):
  - **list structural shapes**: `list.len/get/push/contains/index_of` are
    element-generic at MIR level (verified empirically on `List<String>`),
    so the checker types them with element-aware shapes — `push(xs, x)`
    checks `x` against the receiver's element type and a real mismatch
    (`push(List<String>, 1)`) is E0308; `get` returns `Option<elem>`.
    `sum/max/min` are genuinely Int-only and checked strictly. Guarded on
    the merged declaration matching the known stdlib shape so a user module
    named `list` is not misrouted.
  - **per-pair container fallback** (other modules): when one argument
    disagrees with the declared type only in the element type under the
    same container head, only that pair is skipped — every other argument
    is still checked, and the call types as `Ty::Error` rather than the
    Int-specialised declared return. Narrows once stdlib signatures become
    honestly generic.
- **Builtin modules are typed too** (review fix): `core.sys`
  (`args() -> List<String>`, `env(String) -> Option<String>`,
  `exit(Int) -> Never`) and `core.tasks` (`join_all`/`select`, silent —
  generic, typed at MIR) get their spec §17 surface in the checker, and
  typos on them get the same E0318 as merged modules.
- For merged modules, E0318 fires only when at least one `{m}__*` symbol
  exists; pipelines that skip `resolve_imports` keep the silent fallback.

Regression gate at landing: `bench/static-corpus/` 76 passed / 0 failed
(including new `bad/E0305-module-call-string-concat.ty` and
`bad/E0318-unknown-module-fn.ty`), all examples check clean, workspace
`cargo test` green, ai-gen cases 053/100 now fail in `check` with E0305
instead of crashing codegen.

## Consequences

- E0305 and friends fire where they always should have; the inkwell ICE
  class "PointerValue but expected IntValue" disappears for this cause.
- The checker/MIR division of labour becomes explicit: the checker owns
  user-facing type errors; MIR lowering's typing becomes an internal
  consistency layer (long-term: assert-only).
- `check()`'s signature changes — single-file callers (tests) pass an empty
  map; no behavioural change for files without imports.
- Some hand-written special cases become redundant once signatures cover
  them; they are removed only where the special case adds nothing beyond
  the signature (each removal individually verified).
- Compile time: one map lookup per module call — negligible.

## Alternatives considered

| Option | Rejected because |
|---|---|
| Hand-written signature table in `checker.rs` | Drifts from stdlib reality; every stdlib PR must touch the checker; the bug class returns silently |
| Build-script-generated table from `stdlib/*.ty` | Solves stdlib but not user modules; adds a codegen step; AST collection at check time is simpler and uniform |
| Full cross-module type checking (check every imported module's bodies) | Right long-term, far larger scope; signature threading delivers the user-visible fix now |
| Status quo + more special cases | The escape hatch remains; every uncovered function is a silent hole |
| Keep `Ty::Error` but warn | A warning for "this expression is not type-checked" is an admission, not a fix |
