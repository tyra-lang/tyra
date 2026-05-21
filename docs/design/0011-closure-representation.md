# ADR 0011: Closure representation and lambda C ABI

- **Status**: Accepted
- **Date**: 2026-05-21
- **Spec sections affected**: §9.4 (function types and anonymous functions)

## Context

Spec §9.4 defines first-class anonymous functions (lambdas) with the following
capture rules:

- Captures are **read-only by default**
- `mut` binding rebinding from inside a closure is **forbidden**
- `value` captures are **semantically copied** into the closure
- `data` captures are treated as **references** (GC-managed pointer)

As of v0.3.0, lambdas are fully parsed (`tyra-parser/src/decl.rs:90`),
represented in the AST (`tyra-ast/src/types.rs:321` `ExprKind::Lambda`,
`:502` `Ty::Fn`), type-checked (`tyra-types/src/checker.rs:1437`), and handled
in monomorphization (`tyra-mir/src/monomorphize.rs:225`). However, MIR lowering
stubs out every lambda as `Constant::Unit` (`tyra-mir/src/lower/expr.rs:704`),
and there is no closure handling anywhere in the LLVM codegen. Landing lambdas
requires a runtime representation, a calling convention, a free-variable
analysis pass, and a new type-checker diagnostic for the `mut` rebinding
prohibition.

## Decision

### 1. Runtime representation — uniform fat pointer

Every function value (lambda or named function passed as a value) is represented
as a **2-word fat pointer** at runtime:

```
struct ClosureVal {
    fn_ptr:  *const u8,   // pointer to the lifted LLVM function
    env_ptr: *mut u8,     // pointer to the GC-allocated environment struct
}
```

Non-capturing lambdas use `env_ptr = null`. Named functions used as values are
wrapped in a fat pointer whose `fn_ptr` points to a **static thunk** (a
compiler-generated wrapper function that ignores its `env_ptr` and calls the
named function directly); `env_ptr = null`.

Uniform fat pointers eliminate a separate "bare function pointer" type visible
to the user and make higher-order functions (e.g., `map`) simpler to codegen:
every `fn(T) -> U` argument slot always receives a fat pointer.

### 2. C ABI — `env_ptr` as the first argument

Every **lifted lambda function** in LLVM IR takes `env_ptr: i8*` as its
**first argument**, followed by the user-visible parameters:

```llvm
; fn(_ x: Int) -> Int  with env_ptr first
define i64 @lambda_0(i8* %env, i64 %x) { ... }
```

Call sites extract `fn_ptr` and `env_ptr` from the fat pointer, then call:

```llvm
%result = call i64 %fn_ptr(i8* %env_ptr, i64 %arg)
```

Named function thunks follow the same signature (their `env` argument is
unused). This means all function values share a uniform call protocol.

### 3. Free variable analysis — in MIR lowering

Free variable collection is performed in **`tyra-mir/src/lower/`** (during MIR
lowering of `ExprKind::Lambda`), not in the resolver or type checker.

Rationale:
- The resolver handles name → definition mapping; adding capture-set semantics
  would couple it to value-level concerns it does not currently track.
- The type checker is already responsible for inferring and checking types; it
  should not also be responsible for closure lifting.
- MIR lowering already has access to the full expression tree, the current
  variable environment, and the `TypeIndex` (for determining `value` vs `data`
  per captured variable). This is the natural place to decide the shape of the
  environment struct.

The free variable set is: all **resolved bindings** referenced in the lambda
body that originate from an **enclosing scope** — i.e., not lambda parameters
and not `let`/`mut` bindings introduced within the lambda body itself.

Analysis operates on **resolved binding identity** (the unique binding obtained
from the lexical scope stack at the point of reference), not on identifier name
strings. This correctly handles shadowing: if an inner `let x` shadows an outer
`mut x`, a reference to `x` inside the lambda resolves to the inner binding
(local, not captured), not the outer one.

The capture set is ordered by **lexical first-use order** — the order in which
each distinct enclosing binding is first referenced during a depth-first walk of
the lambda body AST. This ordering is deterministic and reproducible across
compilation runs, and must be applied consistently at both the closure creation
site (env struct write) and inside the lifted function body (env struct read).

### 4. Environment struct layout

For each lambda with a non-empty capture set, a GC-allocated environment struct
is emitted:

- **`value`-typed captures** (spec §9.4): the value is **copied** into the
  struct field at closure creation time. The copy matches the usual `value`
  copy semantics in the language.
- **`data`-typed captures** (spec §9.4): a **GC-managed pointer** to the
  original heap-allocated object is stored. This is consistent with `data`
  reference semantics elsewhere in the language.

Struct fields are laid out in **lexical first-use order** (as defined in §3
above). The struct is allocated with `GC_malloc` (consistent with ADR 0007).

### 5. Frontend types — `Ty::Fn` unchanged; no new MIR type enum

`Ty::Fn(Vec<Ty>, Box<Ty>)` in `tyra-ast/src/types.rs` continues to be the type
of any function value. There is no user-visible distinction between a closure
and a plain function pointer at the Tyra type level.

The MIR IR (`tyra-mir/src/ir.rs`) currently carries `tyra_types::Ty` directly on
`Function::params`, `Function::return_type`, `StructDef::fields`, etc., and has
no separate `MirTy` enum. **No new MIR type enum is introduced by this ADR.**
Closure values are instead represented by two new MIR `Instruction` variants and
a new `StructDef` (the anonymous env struct) that are emitted during lambda
lowering:

- `Instruction::ClosureBuild { dest, fn_name, env_fields }` — constructs the
  2-word fat pointer from a lifted function name and a list of captured operands.
- `Instruction::IndirectCall { dest, fn_ptr, env_ptr, args }` — calls through a
  fat pointer's `fn_ptr` field with `env_ptr` prepended.
- An anonymous `StructDef` (`__closure_env_<id>`) is added to `Program::struct_defs`
  for each lambda with a non-empty capture set; its fields carry `Ty` values for the
  captured variables.

The carried `Ty` on all these constructs remains `Ty::Fn(params, ret)` for the fat
pointer itself and the captured types from the existing frontend type. No change to
`tyra_types` is required.

Monomorphization (`tyra-mir/src/monomorphize.rs:225`) already substitutes type
parameters in lambda parameter and return types. This remains correct: lambda
type-level monomorphization happens before MIR lowering; the fat pointer lifting
happens inside the lowering pass.

### 6. New diagnostic — E0402: cannot rebind `mut` capture

Spec §9.4 forbids rebinding a `mut` binding from an enclosing scope inside a
closure. This check is added to the **type checker** (`tyra-types/src/checker.rs`)
during lambda body checking, using the existing scope tracking pattern.

Error code: **`E0402`** (`E0400` = match non-exhaustive, `E0401` = nested
constructor exhaustiveness, `E0402` = closure `mut`-rebind violation).

Example:
```tyra
mut counter = 0
let f = fn() -> Unit
  counter = 1   # error[E0402]: cannot assign to `counter` captured by closure
end
```

## Alternatives considered

### A. Two distinct types: `Ty::FnPtr` (bare) vs `Ty::Closure` (fat pointer)

A non-capturing lambda could be a bare C function pointer (no env overhead),
while a capturing lambda would be a fat pointer. This requires a coercion rule
whenever a `FnPtr` is passed where a `Closure` is expected, and complicates both
the type checker and the call sites.

**Rejected**: the complexity of the coercion rule outweighs the allocation
savings of avoiding the env struct for non-capturing lambdas. The `env_ptr =
null` optimization (§Decision 1) achieves the same allocation saving without
any type-level distinction.

### B. Free variable analysis in the resolver

The resolver already performs scope walking and could annotate each lambda with
its capture set. This would make capture information available to the type
checker.

**Rejected**: the resolver does name-to-definition binding, not value-capture
semantics. Adding capture-set tracking to the resolver would conflate two
responsibilities. The `TypeIndex` available at MIR lowering time is sufficient
to determine `value`/`data` capture kinds.

### C. Free variable analysis in the type checker

The type checker walks the lambda body and has access to the environment; it
could annotate `ExprKind::Lambda` nodes with capture sets stored in a side
table.

**Not rejected outright, deferred**: this is viable and would make capture
metadata available earlier (useful if we later need it for type-level effects).
For v0.4.0, MIR lowering is simpler because the lowering pass already needs to
reconstruct the env struct anyway. If a future pass needs capture information
before MIR, revisit as an amendment to this ADR.

### D. Named functions passed as values always remain bare function pointers

A named function reference (`foo` in `map(list, foo)`) could be passed as a raw
`fn_ptr` if the callee checked its argument type. This requires two call
protocols and special-casing at every call site.

**Rejected**: uniform fat pointers (§Decision 1) keep all call sites identical
and require no special-casing. The static thunk overhead is negligible.

## Consequences

**Positive**

- Lambdas become first-class values; `map`/`filter`/`fold` and any user-defined
  higher-order function become expressible.
- Uniform fat pointer means all `fn(T) -> U` arguments are called identically,
  regardless of whether the value is a non-capturing lambda, a capturing closure,
  or a named function. No call-site branching.
- `Ty::Fn` at the frontend is untouched; the spec type system requires no change.
- GC already manages env struct memory; no new allocation strategy needed.

**Negative / accepted tradeoffs**

- Every function value is 2 words (fat pointer) rather than 1 word (bare
  pointer). For workloads that pass many non-capturing functions, this doubles
  the value size. Acceptable at v0.4.0 scale.
- Every indirect call passes an unused `env_ptr = null` for non-capturing
  lambdas. A future optimizer could eliminate this, but it is not needed now.
- Static thunks are emitted per named function used as a value. The number of
  such thunks is bounded by the number of use sites; no combinatorial blowup.

**Reversibility**

All closure-related code is isolated to three layers:
1. MIR lowering (`tyra-mir/src/lower/expr.rs`, `call.rs`) — free variable
   analysis and env struct construction.
2. LLVM codegen (`tyra-codegen-llvm/src/codegen.rs`, `instr_emit.rs`) — fat
   pointer construction and indirect call emission.
3. Runtime (no new runtime functions needed; `GC_malloc` already in place via
   ADR 0007).

If a future release switches to a different closure representation (e.g.,
separate thin pointer + out-of-band env, or precise GC stack-allocated
closures), all changes are contained in those two crates plus this ADR
(superseded). No language-visible semantics change.

## Implementation order (for Phase B)

1. **Type checker** (`checker.rs`): add E0402 diagnostic for `mut` rebind in
   lambda body.
2. **MIR lowering** (`lower/expr.rs:704`): replace `Constant::Unit` stub with
   free variable analysis + env struct construction + lifted function + fat
   pointer build instructions.
3. **MIR lowering** (`lower/call.rs`): distinguish fat-pointer indirect call
   from direct named-function call.
4. **LLVM codegen** (`codegen.rs`, `instr_emit.rs`): emit lifted functions,
   env struct `GC_malloc`, fat pointer construction, indirect call via `fn_ptr`.
5. **Static thunk emission** for named functions used as values.
6. **Tests**: corpus programs (non-capturing lambda, value capture, data
   capture, E0402 negative case).
