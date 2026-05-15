# ADR 0002: Float does not have the Eq ability

- **Status**: Accepted
- **Date**: 2026-04-15
- **Spec sections affected**: §7.2, §8.4, §8.6, §17

## Context

Tyra's ability system auto-derives `Eq` for `value` and `data` types when all fields satisfy `Eq`. `Eq` enables `==` and `!=` and is required for `Set<T>` and `Map<K, V>` keys.

IEEE 754 floating-point defines that `NaN != NaN`. This creates a fundamental conflict:

```tyra
value Point
  x: Float
  y: Float
end

let p = Point(x: 0.0 / 0.0, y: 1.0)  # x is NaN
p == p  # Should this be true (structural) or false (IEEE 754)?
```

If `Float` has `Eq`:

- Structural equality says `p == p` is `true` (all fields are identical bits)
- IEEE 754 says `p == p` is `false` (NaN != NaN)
- This is a contradiction that cannot be resolved without violating one invariant

If `Float` does not have `Eq`:

- `Float == Float` is not directly available
- `Point` (with Float fields) does not auto-derive `Eq`
- Users must use explicit comparison functions
- The contradiction is avoided entirely

## Decision

**`Float` does not have the `Eq` ability.**

Consequences for the type system:

- `Float == Float` is a compile error
- `Set<Float>` is a compile error (requires `Hash`, which requires `Eq`)
- `Map<Float, V>` is a compile error
- Any `value` or `data` with a `Float` field does not auto-derive `Eq` or `Hash`
- Explicit comparison is available via standard library functions

```tyra
# Compile error
let a: Float = 1.0
if a == 1.0    # error: Float does not satisfy Eq
  ...
end

# Correct: use explicit comparison
import float

if float.eq(a, 1.0)
  ...
end

# Or with tolerance
if float.approx_eq(a, 1.0, epsilon: 0.001)
  ...
end
```

### Float does have `Ord`

Unlike `Eq`, `Ord` for `Float` is less problematic in practice. IEEE 754 defines a total order for non-NaN values, and Tyra can define `Ord` for `Float` with the convention that NaN comparisons return `false` (consistent with IEEE 754's `<`, `>`, `<=`, `>=`).

However, since `Ord` for `value` types is only auto-derived for single-field types (spec §8.6), `value Point { x: Float, y: Float }` still does not get `Ord` regardless.

### Standard library support

The `float` module in the standard library provides:

```tyra
fn eq(_ a: Float, _ b: Float) -> Bool            # IEEE 754 numeric equality (NaN == NaN → false; 0.0 == -0.0 → true)
fn approx_eq(_ a: Float, _ b: Float, _ eps: Float) -> Bool
fn is_nan(_ x: Float) -> Bool
fn is_infinite(_ x: Float) -> Bool
```

## Consequences

### What becomes easier

- No silent bugs from `NaN == NaN` returning unexpected results
- The ability system remains simple (no `PartialEq` / `Eq` split)
- `HashMap<Float, V>` bugs (common in Python, JavaScript) are impossible
- AI code generation cannot produce the `NaN == NaN` trap

### What becomes harder

- Comparing two Float values requires an explicit function call
- `value` types with Float fields cannot be used in `Set` or as `Map` keys
- Pattern matching on Float literals is not possible (no `Eq`)
- Unit testing Float results requires `float.approx_eq` instead of `==`

### Practical impact assessment

Tyra's target use cases (web backends, CLI tools, business apps) rarely need Float equality:

- **Web backends**: JSON numbers are typically compared as strings or integers
- **CLI tools**: Rarely involve floating-point comparison
- **Business apps**: Money should use `Int` (cents), not `Float`
- **Data processing**: Explicit epsilon comparison is the correct approach anyway

The inconvenience is real but confined to a narrow domain. Users who genuinely need Float equality (scientific computing, graphics) are outside Tyra's v0.1 target (spec §4).

## Alternatives considered

### A. Float has Eq with IEEE 754 semantics (NaN != NaN)

This is the C, Java, JavaScript, Python approach.

Rejected because:

- Breaks the invariant that `Eq` is reflexive (`a == a` is always `true`)
- Breaks `value` auto-derivation: `Point { x: NaN, y: 1.0 } == Point { x: NaN, y: 1.0 }` is `false`, which violates the "structural equality" promise
- Creates a class of subtle bugs that Tyra's design principles aim to eliminate

### B. Float has Eq with NaN == NaN being true

Rejected because:

- Violates IEEE 754, surprising to anyone with numerical computing experience
- `HashMap` with Float keys would work but produce mathematically incorrect results
- No mainstream language does this, so it would be a unique footgun

### C. Introduce PartialEq as a separate ability

This is the Rust approach (`PartialEq` vs `Eq`).

Rejected because:

- Adds a 5th ability, increasing conceptual overhead
- The ability system was deliberately simplified (ADR discussion in spec review round 6)
- `PartialEq` vs `Eq` is one of Rust's most confusing distinctions for beginners
- Contradicts the "less strict than Rust" goal

### D. Float has Eq, but value types with Float fields don't auto-derive

A hybrid approach: `1.0 == 1.0` works, but `Point == Point` doesn't auto-derive.

Rejected because:

- Inconsistent: `Float` satisfies `Eq` but doesn't propagate to containing types
- The auto-derivation rule "all fields satisfy Eq → type has Eq" would need an exception
- Exceptions to simple rules are the enemy of AI-friendly design

## References

- Spec §7.2 (Primitive types)
- Spec §8.6 (value and data, ability auto-derivation)
- IEEE 754-2019, Section 5.11 (Comparison predicates)
- Rust `PartialEq` vs `Eq`: <https://doc.rust-lang.org/std/cmp/trait.Eq.html>
- Phase 0a: SPEC_GAP K (Float Eq and NaN)
