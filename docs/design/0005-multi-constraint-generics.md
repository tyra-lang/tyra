# ADR 0005: Allow up to two constraints per type parameter

- **Status**: Accepted
- **Date**: 2026-04-15
- **Spec sections affected**: §8.4
- **Related**: ADR-0001 (ADT semantics), ADR-0003 (stdlib scope)

## Context

The original v0.1 specification limited each type parameter to at most one constraint:

> Each type parameter may have 0 or 1 constraints
> v0.1 allows only the simple form `<T: Constraint>`
> `where` clauses, multiple constraints, associated types, and higher-kinded types are not adopted

This was a deliberate simplification to keep the type checker simple, error messages clear, and the AI generation surface narrow.

During Phase 0b reviews, the language design expert flagged this as **a near-fatal omission** for practical use. The fundamental problem:

```tyra
# Cannot be written with single-constraint generics
fn deduplicate<T: Eq + Hash>(_ items: List<T>) -> List<T>
  ...
end
```

The standard library cannot implement basic operations without multi-constraint support:

- `Set<T>` requires `T: Eq + Hash` (to deduplicate by value)
- `Map<K, V>` requires `K: Eq + Hash` (for key lookup)
- `List.unique` requires `T: Eq + Hash`
- `List.sort` requires `T: Ord` (which implies `T: Eq`, so this is OK)
- Generic JSON deserialization requires `T: Debug + FromJson`

With only single-constraint generics:

- The Tier 1 standard library (`core`, `core.sys`, `core.tasks`) can be written
- But the Tier 2 standard library (`collections`, `json`, etc.) cannot
- Users writing libraries hit this limit immediately

The reviewer's exact wording:

> Map 系ユーティリティが書けない。標準ライブラリ内部で詰む。trait 設計が歪む。
> v0.1 でも 2 つまでは許可すべき。

The decision is: do we accept this limit and ship a crippled v0.1, or do we relax the rule slightly?

## Decision

**Allow up to two constraints per type parameter, joined with `+`.**

The new syntax:

```tyra
fn deduplicate<T: Eq + Hash>(_ items: List<T>) -> List<T>
  ...
end
```

Specifically:

- Each type parameter may have **0, 1, or 2** constraints
- Constraint syntax is `<T: Constraint>` for one, or `<T: A + B>` for two
- Each `Constraint` may be either a trait or an ability
- The `+` is left-associative but order does not affect semantics
- Three or more constraints, `where` clauses, associated types, and higher-kinded types remain unsupported

### Why exactly two

The choice of "up to two" is deliberate, not arbitrary:

1. **Two is enough for the standard library.** The most common pair is `Eq + Hash` (for hash-based collections). `Ord` implies `Eq` ergonomically. `Debug + Stringable` could appear, but practical APIs rarely need three.

2. **Two preserves implementation simplicity.** The constraint solver remains a simple intersection of two sets. With three or more, users start expecting `where` clauses for readability, and the slippery slope toward Rust-style trait bounds begins.

3. **Two is a clean cliff.** "Up to two" is a memorable rule. "Up to three" raises the question "why not four?" — and there is no good answer. Two is the smallest number that solves the practical problem.

4. **Two avoids the "why not three?" rabbit hole.** Setting the limit at two creates an obvious need for `where` clauses at three or more, which can be added in v0.2 with proper design rather than reactive expansion.

## Consequences

### What becomes possible

- The Tier 2 standard library (`collections`, `set`, `map`) can be implemented in Tyra itself
- User-defined hash-based data structures work with generics
- Generic helpers like `unique<T: Eq + Hash>`, `frequency<T: Eq + Hash>` work
- Common patterns from Rust, Swift, Kotlin transfer cleanly

### What remains harder

- Three-constraint cases require `trait` aggregation:

  ```tyra
  # If Eq + Hash + Debug were needed:
  trait HashableDebug
  end
  
  impl<T: Eq + Hash + Debug> HashableDebug for T  # not allowed in v0.1
  ```

  This is intentional. Users hitting this limit are pushed toward defining a trait, which is usually clearer anyway.

- `where` clauses are not available for verbose constraint lists
- Associated types are not available (e.g., `T: Iterator<Item = U>`)
- Higher-kinded types are not available

These remain in §22 (deferred items) for v0.2 or later.

### Impact on the type checker

The change is small but non-trivial:

- The constraint position in the AST becomes a `Vec<Constraint>` with len ∈ {0, 1, 2} instead of `Option<Constraint>`
- Constraint resolution becomes a set intersection check instead of a single check
- Error messages need updating to handle "type T does not satisfy A + B" cases
- The `+` token gains a new context (in addition to numeric addition)

Estimated implementation effort: a few days of work, well within Phase 1.

### Impact on parser

`+` in constraint position is unambiguous because it can only appear inside `<...>`. The parser can distinguish:

```tyra
fn f<T: A + B>(...)   # constraint context: + is constraint join
let x = a + b          # expression context: + is numeric addition
```

The contexts are distinguished by the surrounding tokens (`<`, `>`, `:` for constraints; otherwise expression). No ambiguity arises.

### Impact on AI generation

Slightly more surface area to learn, but:

- The pattern `<T: Eq + Hash>` is heavily represented in Rust training data
- AI models will generate this syntax naturally
- The alternative (no multi-constraint) would force AI to invent workarounds, which is worse

## Alternatives considered

### A. Keep single-constraint only

Stick with the original rule and require users to define helper traits for combinations.

**Rejected** because:

- The standard library cannot be implemented
- Helper traits like `EqAndHash` are noise that exists only to work around the language
- AI models trained on Rust will repeatedly violate this rule
- Three independent reviewers flagged this as critical

### B. Allow unlimited constraints

`<T: A + B + C + D>` permitted with no limit.

**Rejected** because:

- The "why not where clauses?" pressure becomes immediate
- Long constraint lines are visually noisy in function signatures
- Once 4+ constraints appear, the function probably needs refactoring anyway
- This is closer to Rust's approach, which Tyra explicitly tries to avoid

### C. Adopt `where` clauses immediately

Allow `fn f<T>(...) where T: Eq + Hash + Debug, U: Default { ... }`.

**Rejected** because:

- `where` clauses are syntactically heavy for simple cases
- They require deciding on placement (before `{`, after return type, etc.)
- They often come bundled with type-equality bounds (`where T::Item = U`), which Tyra doesn't have
- v0.1 should ship with the smallest viable feature set; `where` is a v0.2 conversation

### D. Allow exactly two and never expand

Hard-cap at two forever.

**Rejected** because:

- Users with three-constraint needs are stuck forever
- Eventually `where` clauses or some equivalent will be needed
- "Two for v0.1, more in v0.2 with proper design" is more honest than "two forever"

### E. Use intersection types `<T: A & B>`

TypeScript-style intersection.

**Rejected** because:

- `&` already conflicts with potential bitwise-and (if Tyra adds bit operations)
- `+` is the established convention for trait bounds in Rust, Swift, Scala
- AI models recognize `+` for this purpose
- No semantic difference from `+`, just notation churn

## Migration notes

For code written against the original single-constraint spec:

```tyra
# Before (workaround with helper trait)
trait Lookupable
end

impl Lookupable for X
  ...
end

fn lookup<T: Lookupable>(...) -> T

# After (direct multi-constraint)
fn lookup<T: Eq + Hash>(...) -> T
```

Helper traits defined solely as constraint aggregators can be removed.

## Future direction

When v0.2 is designed, the natural next steps are:

1. Three or more constraints (likely with `where` clauses for readability)
2. `where` clauses with type-equality bounds (associated types)
3. Higher-kinded types (much later, if at all)

Each of these should have its own ADR and discussion. ADR-0005 deliberately stops at two to preserve design space.

## References

- Spec §8.4 (Generics)
- Phase 0b Reviews:
  - Language design reviewer (Section 1.2: "Generics 制約 (致命的に近い)")
  - Compiler implementation reviewer (implicit, in trait/ability discussion)
- Rust trait bounds: <https://doc.rust-lang.org/book/ch10-02-traits.html#traits-as-parameters>
- Swift generic constraints: <https://docs.swift.org/swift-book/documentation/the-swift-programming-language/generics/>
- Scala context bounds: <https://docs.scala-lang.org/scala3/book/types-introduction.html>
