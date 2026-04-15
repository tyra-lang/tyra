# AGENTS.md

This file provides guidance to Claude Code when working with the Tyra language project.

## Project Overview

Tyra is a statically typed, AI-friendly programming language designed for backend services, CLI tools, and business applications. It compiles to native binaries via LLVM.

**Current stage**: Pre-alpha. Implementing Language Spec v0.1.

**Core design principles** (from spec §2):

1. Explicitness over implicitness — no `null`, no truthy/falsy, no implicit conversions
2. Single-interpretation syntax — the same input must produce the same AST
3. AI-friendly — design choices favor LLM completion stability over expressive shortcuts
4. Practical static typing — `Option` and `Result` are first-class
5. Simple operations — unified toolchain, single-binary deployment

When in doubt about any design decision, **refer to `docs/spec/ja/language-spec.md`**. The specification is the source of truth.

## Repository Structure

```text
tyra/
├── docs/spec/ja/language-spec.md  # Language specification (Japanese, authoritative)
├── docs/spec/en/                  # English translation (may lag)
├── docs/design/                   # ADRs (Architecture Decision Records)
├── docs/rfcs/                     # Future change proposals
├── compiler/                      # Rust workspace for the compiler
│   └── crates/
│       ├── tyra-lexer/
│       ├── tyra-parser/
│       ├── tyra-ast/
│       ├── tyra-types/            # Type checker, ability checking, trait resolution
│       ├── tyra-resolve/          # Name resolution, modules
│       ├── tyra-mir/              # Mid-level IR
│       ├── tyra-codegen-llvm/     # LLVM backend
│       ├── tyra-diagnostics/      # Error messages (i18n, default English)
│       ├── tyra-driver/           # Compilation pipeline
│       └── tyra-cli/              # tyra command
├── stdlib/                        # Standard library, written in Tyra
├── runtime/                       # GC and async scheduler (Rust/C)
├── tools/                         # tyra-fmt, tyra-lsp, tyra-mod
├── tests/conformance/             # Spec compliance tests
├── tests/corpus/                  # Spec by example (real programs)
└── examples/                      # User-facing sample programs
```

## Critical Reading Order

Before making any non-trivial change, Claude should read these in order:

1. `docs/spec/ja/language-spec.md` — entire spec, especially §8 (type system) and §12 (error handling)
2. `docs/design/` — past design decisions and their rationale
3. The relevant crate's `README.md` if present
4. Existing tests in `tests/conformance/` for the area being modified

## Language and Communication

* **Specification document**: Japanese (authoritative)
* **Code comments**: English
* **Identifiers and APIs**: English (ASCII only, per spec §5.1)
* **Error messages**: English by default, i18n via `TYRA_LANG=ja`
* **Commit messages**: English preferred
* **Issues/PRs**: English preferred, Japanese acceptable
* **Conversation with the maintainer**: Japanese

When generating code, comments, and identifiers must be in English. When discussing design or explaining decisions in chat, Japanese is fine.

## Build and Test Commands

Use the repository's documented commands as the source of truth. At minimum, the Rust workspace should support:

```bash
# Build the compiler
cd compiler && cargo build

# Run all compiler tests
cd compiler && cargo test

# Build release binary
cd compiler && cargo build --release
```

For conformance tests, formatter checks, and corpus tests, use the repository's current documented entry points. Keep this file aligned with the actual harness wiring.

When implementing a new feature, **always add a corresponding conformance test** in `tests/conformance/` with a comment referencing the spec section.

## Spec Compliance Rules

These are non-negotiable. Any code that violates them is a bug, not a tradeoff.

### Syntax Rules (spec §5, §6, §9)

* Block delimiter is `end`, not `}`
* Function calls always require parentheses, even with zero arguments
* No truthy/falsy — only `Bool` is allowed in conditions
* No `null` anywhere in the language
* No semicolons; newlines are statement terminators
* No multiline comments
* Identifiers are ASCII only (spec §5.1)

### Type System Rules (spec §8)

* `value` types are immutable by definition; field updates are forbidden
* `data` types may have `mut` fields; updates require both `mut` field **and** `mut` binding
* `Option<T>` is the only way to express absence; never introduce `null` semantics
* Nominal typing only — no structural typing
* Generics use `<T>`, indexing uses `[]`, list literals use `[]`
* Turbofish `parse::<Int>(text)` is required for explicit type application in expression position

### Trait vs Ability Distinction (spec §8.4, §8.6, §8.7, §17)

This is critical and easy to get wrong:

* **Abilities** (`Eq`, `Hash`, `Ord`, `Debug`): compiler-known, inferred from type shape, **cannot be implemented manually with `impl`**
* **Traits** (`Stringable`, `Into<T>`, user-defined): require explicit `impl` blocks, support static dispatch only

When implementing the type checker:

* Automatically determine abilities for `value` and `data` per the rules in §8.6
* `Hash` requires `Eq` (enforce this invariant)
* `data` does **not** automatically provide `Ord`; prefer `sort_by`, `min_by`, and `max_by`
* `value` automatically provides `Ord` only for single-field types
* `Debug` is for debug/log/diagnostic formatting
* `Stringable` is for human-facing formatting

If someone writes `impl Eq for User`, that is an error: `Eq` is an ability, not a trait.

### Error Handling Rules (spec §12)

* `?` works only with `Result<T, E>`, never with `Option<T>`
* `?` requires `E: Into<F>` where `F` is the enclosing function's error type
* `Into<T> for T` is auto-provided by the compiler
* `or return` works only with `Option<T>`
* No exceptions, ever

### Async Rules (spec §14)

* `async fn f(...) -> T` returns `Task<T>` when called
* `.await` is postfix and binds tighter than `?`
* `fetch(id).await?` parses as `(fetch(id).await)?`
* `.await` is only legal inside `async` functions
* `spawn expr` accepts any expression and returns `Task<T>`
* `Task<T>` is a core runtime type used by async lowering

## What NOT to Do

These are common mistakes that contradict Tyra's design:

* **Do not add features from Rust/Swift just because they are convenient.** If it is not in the spec, propose an RFC first under `docs/rfcs/`.
* **Do not introduce trait objects, `dyn Trait`, or any form of dynamic dispatch.** v0.1 is static dispatch only.
* **Do not add structural typing or duck typing.** Tyra is nominal.
* **Do not allow `null` to appear anywhere in error paths or default values.** Use `Option`.
* **Do not add operator overloading.** `+`, `-`, `*`, `/` are built-in for numeric types only.
* **Do not add macros, runtime reflection, or `eval`.** These are explicit non-goals (§3).
* **Do not add multiple constraints to type parameters** (`<T: Eq + Hash>` is forbidden in v0.1; use a single constraint only).
* **Do not add guard clauses to `match`.** Use `if/else` instead. (May be added in a future spec version via RFC.)
* **Do not bypass the spec for ease of implementation.** If the spec is hard to implement, fix the implementation, not the spec.

## When the Spec Is Ambiguous

If you encounter a situation where the spec does not clearly answer the question:

1. **Stop and ask the maintainer** — do not guess
2. Document the ambiguity as a GitHub issue with the `spec-clarification` label
3. The resolution may become a spec patch (v0.1.x) or an RFC for v0.2

Never silently make a design choice that the spec does not endorse. The point of Tyra is predictability.

## Versioning

* **Spec versions**: tagged as `spec-v0.1.0`, `spec-v0.1.1`, `spec-v0.2.0`, ...
* **Compiler versions**: tagged as `v0.1.0`, `v0.1.1`, ...
* The compiler always declares which spec version it implements:

```console
$ tyra --version
tyra 0.1.0
implementing language spec 0.1
```

Spec status is currently **Draft** (v0.1.0). Breaking changes are allowed in MINOR bumps until v1.0.

## Implementation Strategy

Build order (do not skip ahead):

Each step builds on the previous ones. `tyra-diagnostics` comes first because all later crates emit errors through it. `tyra-ast` is separated from the parser because both `tyra-parser` and `tyra-types` depend on it.

1. **Diagnostics** (`tyra-diagnostics`) — error reporting infrastructure; depended on by all subsequent crates
2. **Lexer** (`tyra-lexer`) — tokenize per §5
3. **AST** (`tyra-ast`) — define AST node types consumed by the parser
4. **Parser** (`tyra-parser`) — parse tokens to AST per §6-§14
5. **Name resolution** (`tyra-resolve`) — handle imports, scoping
6. **Type checker** (`tyra-types`) — including ability checking, trait resolution, `?`/`Into` handling
7. **MIR lowering** (`tyra-mir`) — desugar to a stable intermediate form
8. **LLVM codegen** (`tyra-codegen-llvm`) — emit LLVM IR
9. **Runtime** (`runtime/`) — GC and async scheduler in C/Rust
10. **Driver** (`tyra-driver`) — wire compilation phases into a pipeline
11. **CLI** (`tyra-cli`) — expose the driver via command-line interface

Each phase should have:

* Unit tests in the crate
* Integration tests in `compiler/tests/`
* Conformance tests in `tests/conformance/` referencing spec sections

## Code Style for the Compiler (Rust)

* Use `cargo fmt` and `cargo clippy` before every commit
* Prefer `Result<T, TyraError>` over panics for any user-facing path
* Internal invariant violations may use `unreachable!()` or `panic!()`, but document why
* Diagnostic messages must go through `tyra-diagnostics` for i18n
* Spec references in code: `// spec §8.6: value types are immutable`

## Useful Spec Sections by Task

| Task                          | Read first        |
| ----------------------------- | ----------------- |
| Implementing lexer            | §5, §7.3          |
| Implementing parser           | §6, §8.5, §9, §10 |
| Implementing type inference   | §8.1, §8.4        |
| Implementing abilities        | §8.4, §8.6, §17   |
| Implementing traits           | §8.7, §17         |
| Implementing `?` operator     | §12.2             |
| Implementing async            | §14               |
| Implementing pattern matching | §8.5, §10.3       |
| Implementing modules          | §13               |
| Implementing the formatter    | §20               |
| Implementing GC/runtime       | §15               |

## Examples to Reference

When implementing parser or codegen, refer to canonical examples:

* `docs/spec/ja/language-spec.md` §21 (worked examples in the spec)
* `tests/corpus/` (spec by example)
* `examples/hello/main.tyra` (minimal program)

These are the ground truth for what Tyra code looks like.

## Out of Scope (Do Not Implement)

These are explicit non-goals for v0.1 (spec §3, §22):

* Ownership/borrow checker
* Macros
* Operator overloading
* Trait objects, `dyn Trait`
* Runtime reflection
* Actor model (language-level)
* Foreign function interface (FFI) details
* Task cancellation
* Raw strings, multi-line strings
* Guard clauses in `match`
* Where clauses, multiple constraints, associated types
* Higher-kinded types

If a user requests these, point them to the relevant section of §22 and offer to draft an RFC for v0.2.

## Asking for Help

When stuck or uncertain:

* For spec questions: open an issue with `spec-clarification` label
* For implementation questions: open an issue with `implementation` label
* For design proposals: draft an RFC in `docs/rfcs/`

Do not invent answers. Tyra's value depends on consistency; one wrong call can cascade.
