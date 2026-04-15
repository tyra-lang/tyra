# ADR 0004: Remove `or return` and extend `?` to Option

- **Status**: Accepted
- **Date**: 2026-04-15
- **Spec sections affected**: §12.1, §12.2, §10.1, §11
- **Supersedes**: portions of the original §12.2 design

## Context

The original v0.1 specification adopted an asymmetric design for early-return syntax:

- `?` was reserved exclusively for `Result<T, E>`
- `Option<T>` used a separate construct, `or return`

```tyra
# Result early return
fn load_user(_ id: Int) -> Result<User, AppError>
  let row = db.find(id)?
  decode_user(row)?
end

# Option early return (the original design)
fn user_name(_ id: Int) -> Option<String>
  let user = repo.find(id) or return None
  Some(user.name)
end
```

The reasoning at the time was philosophical: `Option` represents *absence*, while `Result` represents *failure*, and these concepts deserved distinct syntactic treatment to reinforce their semantic difference.

During Phase 0a (spec by example) and subsequent expert reviews, this asymmetry was identified as a problem from three independent perspectives:

1. **Compiler implementation perspective**: `or return` requires special parser and type-checker handling. The `or` keyword serves dual roles (logical OR vs. early-return marker), distinguished only by whether `return` follows. This is a parser special case, not a clean syntactic rule.

2. **AI code generation perspective**: LLMs are heavily trained on `?` patterns from Rust. When generating Tyra code, models will naturally write `repo.find(id)?` for Option lookups. Forcing `or return` for Option creates a category of mistakes that AI models will repeatedly produce.

3. **Language design perspective**: The Option/Result asymmetry is unnatural for users. Both `Some(value)` and `Ok(value)` mean "we have a value to continue with"; both `None` and `Err(e)` mean "we don't, return early." Treating them with completely different syntax obscures this structural similarity.

All three reviewers (compiler implementer, AI workflow expert, language design expert) independently recommended unifying the propagation operator.

## Decision

**Remove `or return` from the language and extend `?` to work on both `Result` and `Option`.**

The unified rules are:

### `?` on `Result<T, E>`

- Usable when the expression has type `Result<T, E>`
- The enclosing function's return type must be `Result<U, F>`
- `E` must implement `Into<F>`
- Evaluates to `value` when the result is `Ok(value)`
- Returns `Err(e.into())` early when the result is `Err(e)`

### `?` on `Option<T>`

- Usable when the expression has type `Option<T>`
- The enclosing function's return type must be `Option<U>`
- Evaluates to `value` when the option is `Some(value)`
- Returns `None` early when the option is `None`

### Removed

- `or return` syntax (entirely)
- The `or return` paragraph in §12.2
- The `or return` mention in the §10.1 logical operators section

## Consequences

### What becomes easier

- One propagation operator to learn instead of two
- AI-generated code is more likely to compile on the first try
- Mental model matches Rust, Swift (for Optionals via try?), and other influences
- Users do not need to memorize "Option uses `or return`, Result uses `?`"
- Parser is simpler: `or` becomes purely a logical operator

### What becomes harder

- The Option/Result distinction must be conveyed through types alone, not syntax
- Mixing Option and Result in one function requires explicit conversion
  - e.g., `repo.find(id).ok_or(NotFound)?` to convert `Option<User>` to `Result<User, AppError>`
- The original design's pedagogical clarity ("absence vs. failure") is lost in syntax
  - This is mitigated by Tyra's strong type system: the type tells the story

### Impact on existing examples

The following examples in the spec were updated:

- §11 collection access: `items.get(0) or return None` → `items.get(0)?`
- §12.2 `or return` paragraph: removed entirely
- §10.1 logical operators: removed mention of `or return` parsing rules

### Impact on the prelude

No change. `or` remains a reserved word as a logical operator. The reserved word list in §5.2 is unchanged.

## Alternatives considered

### A. Keep the original asymmetric design

Continue with `?` for Result, `or return` for Option.

**Rejected** because three independent reviewers identified this as a significant friction point. The pedagogical argument was not strong enough to justify the implementation complexity, AI confusion, and learning cost.

### B. Use different operators for Result vs. Option

For example, `?` for Result and `??` for Option, similar to Swift's `try?`.

**Rejected** because:

- It still requires users to remember which operator goes with which type
- AI models are even less likely to choose `??` correctly
- Two operators is more syntactic surface area than one
- The benefit (visually distinguishing Option vs. Result early returns) is small

### C. Remove `?` entirely and use `match` everywhere

This is the most explicit approach: every Result/Option destructuring uses `match`.

**Rejected** because:

- Verbose to the point of harming readability
- Defeats the purpose of having `Result` as the standard error type
- Goes against §2.3 (practical types)
- Ten lines of `match` for what should be one character of syntax

### D. Allow `?` only on the function's exact return type

For example, in a function returning `Result<User, AppError>`, `?` works only on `Result<X, AppError>` (no `Into` conversion).

**Rejected** because:

- This was discussed in earlier spec rounds and was already settled with `Into<F>`
- Removing `Into` would push error conversion boilerplate everywhere
- This change is orthogonal to the Option/Result unification

### E. Unify by lifting Option to Result

Internally treat `Option<T>` as `Result<T, ()>` (Result with Unit error). This is a form of representation unification.

**Rejected** because:

- Tyra has no `()` empty tuple type (Unit is a regular type, not "no error")
- Users would see `Option` and `Result` differently in error messages and documentation
- The conversion would be implicit, violating §2.1 explicitness
- Adds complexity without simplifying the user-facing API

## Migration notes

For anyone who wrote code against the original spec (during the design phase):

```tyra
# Before
let user = repo.find(id) or return None

# After
let user = repo.find(id)?
```

The semantic is identical when the enclosing function returns `Option<T>`. Code that used `or return` with non-None values (such as `or return Err(...)` for early-returning a Result from an Option) must be rewritten using explicit conversion:

```tyra
# Before
let user = repo.find(id) or return Err(AppError.NotFound)

# After
let user = repo.find(id).ok_or(AppError.NotFound)?
```

The standard library `option` module (Tier 2) provides `ok_or` and `ok_or_else` for this conversion.

## References

- Spec §12.1 (Error handling principles)
- Spec §12.2 (Propagation operator)
- Phase 0a Reviews:
  - Compiler implementation reviewer (Section B: "or return is a special case")
  - AI workflow reviewer (Section: "Option early-exit AI compatibility")
  - Language design reviewer (Section 1.5: "Option vs Result asymmetry")
- Rust's `?` operator: <https://doc.rust-lang.org/std/ops/trait.Try.html>
- Swift's `try?`: <https://docs.swift.org/swift-book/documentation/the-swift-programming-language/errorhandling/>
