# ADR 0017: Diagnostic quality for type-mismatch errors (E0308)

- **Status**: Accepted
- **Date**: 2026-05-27
- **Spec sections affected**: none (implementation internal)

## Context

The AI-generated code benchmark (Run 11, `bench/ai-gen/results/SUMMARY.md`) shows that E0308 type-mismatch errors comprise the largest failure bucket at 50 out of 72 compile failures. This represents 69% of the remaining diagnostic volume after earlier improvements closed off parser hallucinations and stdlib undefined-name bugs.

**Current E0308 diagnostic quality**:

The type-checker (`compiler/crates/tyra-types/src/checker.rs:2447–2462`, function `check_type_match`) currently emits:
- A single message: `"type mismatch: expected {expected}, found {actual}"`
- A single label at the **use-site only** (where the type mismatch is detected)
- No secondary label pointing to the **expected-type origin** (function parameter declaration, return type annotation, let binding annotation, etc.)
- No `help` field on `Diagnostic` struct; the `notes` field exists but is never populated for E0308

**Why diagnostics matter for AI code**:

When an AI generator encounters `type mismatch: expected Option<T>, found T`, it has no information about:
- Where the expected type was declared (parameter? return annotation? let binding?)
- Why that type was chosen (language semantics? function signature?)
- How to transform the actual value to match (wrap with `Some`? call a conversion function?)

Current diagnostics force the AI to retry blindly or consult the spec. Richer diagnostics with secondary labels + context-aware help suggestions significantly improve AI auditability and pass-rate recovery on retry.

**Type inference permissiveness constraint**:

The `Ty::Var` type variant (used for type inference placeholders) is intentionally permissive: `types_compatible(Ty::Var, _)` returns `true` for any type, as does `types_compatible(_, Ty::Var)`. This reduces false positives from incomplete type inference but at the cost of missing real errors. Removing this permissiveness would expose many currently-silent mismatches and require mass corpus/benchmark updates — scope explosion deferred to v0.8+.

## Decision

### 1. Add `help` field to `Diagnostic` struct

Modify `compiler/crates/tyra-diagnostics/src/diagnostic.rs`:

```rust
#[derive(Debug, Clone)]
pub struct Diagnostic {
    pub level: Level,
    pub code: Option<String>,
    pub message: String,
    pub labels: Vec<Label>,
    pub notes: Vec<String>,
    pub help: Option<String>,  // NEW
}
```

Add a builder method:

```rust
pub fn with_help(mut self, help: impl Into<String>) -> Self {
    self.help = Some(help.into());
    self
}
```

Update all constructor methods (`error`, `warning`, `note`) to initialize `help: None`.

**Rendering**: The `help` field is rendered as a separate line after all labels and notes, prefixed with `"help: "` (distinct from `notes`). This follows Rust compiler convention.

### 2. Secondary labels: expected-type origin

Modify `check_type_match` to accept an additional parameter: the `Span` of the expected-type declaration (the site where the expected type was declared, not where the mismatch occurred). This span is used to create a secondary label.

**Function signature change**:

```rust
fn check_type_match(
    expected: &Ty,
    actual: &Ty,
    actual_span: Span,           // where the actual value is used
    expected_span: Span,          // where the expected type was declared (NEW)
    report: &mut Report
)
```

**Diagnostic construction** (updated):

```rust
if !types_compatible(expected, actual) {
    let diag = Diagnostic::error(format!(
        "type mismatch: expected {}, found {}",
        expected.display_name(),
        actual.display_name()
    ))
    .with_code("E0308")
    .with_label(Label::new(actual_span, format!("expected {}", expected.display_name())))
    .with_label(Label::new(expected_span, "expected because of this annotation"));
    
    report.add(diag);
}
```

**Call-site updates**: All calls to `check_type_match` must supply both spans:
- When checking a function argument, `expected_span` is the parameter declaration span
- When checking a return statement, `expected_span` is the return type annotation span
- When checking a let binding, `expected_span` is the annotation span

### 3. Four heuristic `with_help` additions (conservative)

Each heuristic **fires only when both types are fully known** — neither side may be `Ty::Var` or `Ty::Error`. This prevents false-positive suggestions when type inference is incomplete.

**Heuristic (i): T vs Option\<T\> / Option\<T\> vs T**

When expected is `Option<T>` but actual is `T`:
```rust
help: "wrap with `Some(...)`"
```

When expected is `T` but actual is `Option<T>`:
```rust
help: "unwrap with `match opt when Some(x) x when None default end`"
```

Note: `.unwrap_or()` does not exist in Tyra v0.6; unwrapping must use match-when syntax.

**Heuristic (ii): T vs Result\<T,E\> when enclosing function returns Result**

When expected is `Result<T, E>` but actual is `T`, **and** the enclosing function's declared return type is also `Result<_, _>`:
```rust
help: "try `expr?` to propagate the error"
```

This heuristic is conservative: it only fires if the function signature already declares a Result return, avoiding false suggestions for non-error contexts.

**Heuristic (iii): Int ↔ Float conversion**

When expected is `Float` but actual is `Int`:
```rust
help: "convert with `float.from_int(x)`"
```

When expected is `Int` but actual is `Float`:
```rust
help: "convert with `float.to_int(x)`"
```

These functions are defined in `stdlib/float.tyra:14` and available in all Tyra programs.

**Heuristic (iv): ADT variant type vs same ADT**

When expected is a concrete ADT variant type (e.g. the type of the `Foo.Bar` constructor) but actual is the parent ADT type itself (e.g. `Foo`), or vice versa:
```rust
help: "did you mean `Foo.Bar(...)`?"
```

Use dot notation (`Foo.Bar`), **not** Rust-style colons (`Foo::Bar`, which is not valid Tyra syntax). Construction syntax in Tyra is `Foo.Bar(...)` with dot notation; patterns use unqualified names like `when Bar(...) ...`.

### 4. Suppress E0308 cascades (checker.rs:1796–1802)

The code block at `compiler/crates/tyra-types/src/checker.rs:1796–1802` currently returns `Ty::Error` to suppress downstream E0308 cascades when an impl method's return type is not yet resolved. This suppression works but is overkill — it hides real type mismatches.

**Change**: Return the **actual type of the impl method** instead of `Ty::Error`, allowing the type-checker to proceed correctly. To prevent cascade floods, add **diagnostic deduplication** to the `Report` struct:
- Track emitted diagnostics by `(span, code)` tuple
- On `report.add(diag)`, check if a diagnostic with the same span and error code was already added
- If yes, discard the duplicate; if no, add it

This allows one E0308 per unique span+code combination while still preserving legitimate distinct mismatches at different sites.

### 5. Ty::Var permissiveness (no change in v0.7.0)

Do **not** attempt to eliminate `Ty::Var` permissiveness by implementing a real type-variable substitution map in v0.7.0. Reason: removing permissiveness would expose many currently-silent mismatches, requiring mass corpus and benchmark updates (scope explosion).

Example: A function that takes `List<T>` with an empty list `[]` argument currently compiles (via `Ty::Var` matching). Strict inference would reject it, requiring the caller to explicitly annotate `[]` as `List<Int>` etc. This would break existing code.

**Deferred decision**: v0.8+ will reconsider full type-variable inference with a proper substitution map. For v0.7.0, heuristic help is the tool to guide AI toward correct fixes without breaking existing permissiveness.

## Alternatives considered

### A. Eliminate Ty::Var permissiveness in v0.7.0

Implement a full type-variable unification with substitution map, reject Ty::Var mismatches. This would produce stricter, more correct diagnostics.

**Rejected**: Mass corpus impact. Removing permissiveness exposes silent mismatches currently hidden by `Ty::Var` compatibility. Benchmark Run 11 shows 50 E0308s; stricter inference might double or triple this count, and all AI-generated code would need updates. Scope explosion. Deferred to v0.8+.

### B. LSP quick-fix suggestions

Extend the Language Server Protocol to emit structured `CodeAction` objects with automated fix suggestions (e.g. "wrap with `Some`" → code snippet `Some($expr)`).

**Rejected (for now)**: LSP machinery not yet integrated. The current harness (`tyra-cli`) emits plain text diagnostics to stderr. QuickFix suggestions require LSP server infrastructure. Deferred to v0.8+ or later.

### C. Always emit help regardless of type certainty

Fire all four heuristics even when `Ty::Var` or `Ty::Error` is present.

**Rejected**: False-positive help is worse than no help. Example: if a function parameter is unannotated and inferred as `Ty::Var`, suggesting `"wrap with Some(...)"` based on a partial heuristic match would mislead the AI into incorrect fixes. Conservative guarding (both types fully known) prevents this.

### D. Single-label design (no secondary expected-type label)

Keep the current structure: one label at the use-site, no secondary label pointing to the expected-type origin.

**Rejected**: Without the secondary label, AI generators still must hunt for the declaration site. Multi-label output is standard in modern compilers (Rust, GHC, Elm) and provides crucial context. Minimal rendering cost for significant information gain.

## Consequences

**Positive**

- E0308 diagnostics now guide the AI generator toward correct fixes with secondary labels showing expected-type origins
- Four targeted heuristics address the most common type-mismatch patterns (Option unwrap, Result propagation, numeric conversions, ADT construction)
- Help text is conservative and only emitted when both types are fully known, preventing false positives
- Diagnostic deduplication prevents cascade floods while preserving distinct legitimate mismatches
- Multi-label layout improves readability and aligns with compiler UX conventions
- Benchmark Run 16 will measure the impact on AI-gen pass rate with improved diagnostics

**Negative / accepted tradeoffs**

- `check_type_match` call-sites must be updated to pass both `actual_span` and `expected_span` — moderate refactoring
- Diagnostic dedup adds runtime bookkeeping (set/map of seen (span, code) pairs)
- Heuristic help is inherently imperfect and may not suit all use cases; refinement may be needed after benchmarking
- The four heuristics do not cover all type mismatches (e.g. struct field type mismatch, higher-order function arg mismatches) — extensible design for future heuristics

**Implementation order**

1. Add `help: Option<String>` field and `with_help` method to `Diagnostic` struct
2. Update diagnostic rendering to display help line after labels and notes
3. Add `(span, code)` deduplication to `Report` struct
4. Refactor `check_type_match` to accept `expected_span` parameter and emit secondary label
5. Update all call-sites to `check_type_match` with both spans (search codebase for `check_type_match` calls)
6. Implement four heuristics in `check_type_match` with guards: `!is_var_or_error(expected) && !is_var_or_error(actual)`
7. Fix checker.rs:1796–1802 to return actual impl method type instead of `Ty::Error`
8. Benchmark Run 16: measure AI-gen pass rate against Run 11 baseline

**Note on Tyra syntax correctness (critical for AI auditability)**

The help texts MUST reference correct Tyra syntax:
- Option unwrap: `match opt when Some(x) x when None default end` (NOT `.unwrap_or(default)`)
- Result propagation: `expr?` operator
- Float conversion: `float.from_int(n)` and `float.to_int(x)` (from stdlib/float.tyra:14)
- ADT construction: `Foo.Bar(...)` with dot notation (NOT `Foo::Bar`, which is not valid Tyra syntax)
- ADT patterns: unqualified names like `when Bar(...)`

Incorrect syntax in help text harms AI code quality and makes benchmarks harder to interpret.
