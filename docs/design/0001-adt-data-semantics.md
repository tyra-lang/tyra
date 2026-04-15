# ADR 0001: ADT uses data (reference) semantics

- **Status**: Accepted
- **Date**: 2026-04-15
- **Spec sections affected**: §8.5, §8.6, §15

## Context

Tyra distinguishes `value` (value type, copied, immutable fields) from `data` (reference type, GC-managed, allows mutation and recursion). The spec defines `type` for ADTs (algebraic data types) but does not specify whether ADTs follow value or data semantics.

This matters because:

- `Option<T>` and `Result<T, E>` are ADTs used pervasively
- Recursive ADTs (trees, linked lists) require reference semantics to avoid infinite size
- Performance characteristics differ significantly between value and reference types
- The choice affects whether ADT variants can hold `data` fields

The question was surfaced during Phase 0a (spec by example) when writing a JSON parser that required recursive ADT (`JsonValue` containing `List<JsonValue>`).

## Decision

**ADTs defined with `type` use data (reference) semantics.**

Specifically:

- ADT instances are GC-managed reference types
- ADTs may contain recursive self-references
- ADTs may hold both `value` and `data` fields
- Assignment and parameter passing share the reference (not copy)
- `===` compares reference identity
- `Eq` ability is available if all variant fields satisfy `Eq` (same rule as `data`)
- `Ord` ability is not automatically derived (same rule as `data`)

```tyra
# This is valid: recursive self-reference via data semantics
type JsonValue =
  | JsonNull
  | JsonBool(value: Bool)
  | JsonNumber(value: Float)
  | JsonString(value: String)
  | JsonArray(items: List<JsonValue>)      # recursive
  | JsonObject(entries: Map<String, JsonValue>)  # recursive
```

## Consequences

### What becomes possible

- Recursive ADTs work naturally without `Box` or indirection
- `Option<LargeStruct>` does not copy the entire struct on every move
- Pattern matching on ADTs shares the matched value, avoiding deep copies
- ADTs can hold `data` fields without semantic mismatch

### What becomes harder or impossible

- Small ADTs like `type Color = | Red | Green | Blue` incur heap allocation even though they could fit in a register
- Comparing two `Option<T>` values requires `Eq` explicitly, not structural equality by default
- Users cannot define "lightweight enums" as value types

### Mitigations

- The compiler may apply escape analysis and stack-allocate ADT instances that do not escape (§15.3). This is an optimization that must not affect semantics.
- For truly lightweight enums, users can use `value` with a tag field as a workaround, though this is unidiomatic.

### Impact on standard library

- `Option<T>` and `Result<T, E>` are data (reference) types
- Standard library APIs that return `Option` or `Result` return references, not copies
- This aligns with how most languages handle sum types in GC environments (Kotlin sealed class, Swift indirect enum, OCaml variants)

## Alternatives considered

### A. ADTs are always value types

This is the Rust approach (`enum` is a value type, `Box` is needed for recursion).

Rejected because:

- Tyra has no `Box` or pointer indirection mechanism
- Recursive ADTs would be impossible without introducing new concepts
- Copying large `Result<BigStruct, Error>` on every `?` propagation would be expensive
- Adds complexity that contradicts Tyra's "less strict than Rust" goal

### B. User chooses with `value type` / `data type`

```tyra
value type Color = | Red | Green | Blue      # value semantics
data type JsonValue = | JsonNull | ...        # data semantics
```

Rejected because:

- Adds a decision point that most users shouldn't need to think about
- Increases the surface area of the type system
- The distinction is an optimization concern, not a semantic one in a GC language
- Violates the "one way to do things" principle

### C. ADTs are value types with compiler-inserted indirection for recursion

The Swift approach (`indirect enum`).

Rejected because:

- Requires a new keyword (`indirect`) or compiler magic
- The mental model becomes "it's a value type, except when it's not"
- In a GC language, the performance benefit of value-type ADTs is smaller than in ARC/ownership languages
- Adds complexity for marginal benefit in Tyra's target use cases (web backends, CLI tools)

## References

- Spec §8.5 (Union / ADT)
- Spec §8.6 (value and data)
- Phase 0a example: `05-json-parsing.tyra` (recursive JsonValue)
- Phase 0a example: `09-error-handling.tyra` (nested Result/Option)
