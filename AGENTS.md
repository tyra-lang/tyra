# AGENTS.md

This file provides guidance to AI coding assistants (Claude Code, Codex, Cursor, Aider, Continue, Cline, etc.) when working with the Tyra language project.

## Project Overview

Tyra is a statically-typed, AI-friendly programming language designed for backend services, CLI tools, and business applications. It compiles to native binaries via LLVM.

**Current stage**: Pre-alpha. Implementing Language Spec v0.1.

**Core design principles** (from spec §2):

1. Explicitness over implicitness — no null, no truthy/falsy, no implicit conversions
2. Single-interpretation syntax — same input must produce same AST
3. AI-friendly — design choices favor LLM completion stability over expressive shortcuts
4. Practical static typing — Option/Result are first-class
5. Simple operations — unified toolchain, single binary output

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
│       ├── tyra-diagnostics/      # Error reporting (foundational)
│       ├── tyra-lexer/
│       ├── tyra-ast/
│       ├── tyra-parser/
│       ├── tyra-resolve/          # Name resolution, modules
│       ├── tyra-types/            # Type checker, ability/trait resolution
│       ├── tyra-mir/              # Mid-level IR
│       ├── tyra-codegen-llvm/     # LLVM backend
│       ├── tyra-driver/           # Compilation pipeline
│       └── tyra-cli/              # `tyra` command
├── stdlib/                        # Standard library, written in Tyra
├── runtime/                       # GC and async scheduler (Rust/C)
├── tools/                         # tyra-fmt, tyra-lsp, tyra-mod
├── tests/conformance/             # Spec compliance tests
├── tests/corpus/                  # Spec by example (real programs)
└── examples/                      # User-facing sample programs
```

## Critical Reading Order

Before making any non-trivial change, AI assistants should read these in order:

1. `docs/spec/ja/language-spec.md` — entire spec, especially §8 (type system) and §12 (error handling)
2. `docs/design/` — past design decisions and their rationale (ADRs 0001-0005)
3. The relevant crate's `README.md` if present
4. Existing tests in `tests/conformance/` for the area being modified

## Language and Communication

- **Specification document**: Japanese (authoritative)
- **Code comments**: English
- **Identifiers and APIs**: English (ASCII only, no Unicode identifiers)
- **Error messages**: English by default, i18n via `TYRA_LANG=ja`
- **Commit messages**: English preferred
- **Issues/PRs**: English preferred, Japanese acceptable
- **Conversation with the maintainer**: Japanese

When generating code, comments, and identifiers must be in English. When discussing design or explaining decisions in chat, Japanese is fine.

## Build and Test Commands

```bash
# Build the compiler
cd compiler && cargo build

# Run all tests
cd compiler && cargo test

# Run conformance tests only
cd compiler && cargo test --test conformance

# Run a single Tyra program (after build)
./target/debug/tyra run examples/hello/main.tyra

# Format check (when formatter exists)
./target/debug/tyra fmt --check stdlib/

# Build release binary
cd compiler && cargo build --release
```

When implementing a new feature, **always add a corresponding conformance test** in `tests/conformance/` with a comment referencing the spec section.

## Spec Compliance Rules

These are non-negotiable. Any code that violates them is a bug, not a tradeoff.

### Syntax Rules (spec §5, §6, §9)

- Block delimiter is `end`, not `}`
- Function calls always require parentheses, even with zero arguments
- No truthy/falsy — only `Bool` is allowed in conditions
- No `null` anywhere in the language
- No semicolons; newlines are statement terminators
- Newlines inside `(...)`, `[...]`, `{...}` are not statement separators
- No multiline comments
- Identifiers are ASCII only

### Logical Operators (spec §10.1)

- Use `and`, `or`, `not` keywords (not `&&`, `||`, `!`)
- Both operands of `and`/`or` must be `Bool`
- Precedence: `not` > `and` > `or`
- `or` is purely a logical operator (no special early-return meaning)

### Type System Rules (spec §8)

- `value` types are immutable by definition; field updates are forbidden
- `data` types may have `mut` fields; updates require both `mut` field AND `mut` binding
- `Option<T>` is the only way to express absence; never introduce `null` semantics
- Nominal typing only — no structural typing
- Generics use `<T>`, indexing uses `[]`, list literals use `[]`
- Type parameters may have 0, 1, or 2 constraints joined by `+` (e.g., `<T: Eq + Hash>`)
- Three or more constraints, `where` clauses, associated types are NOT supported in v0.1
- Turbofish `parse::<Int>(text)` is required for explicit type application in expression position
- ADTs use **data semantics** (reference type, GC-managed) — see ADR-0001
- ADT constructors are called as `TypeName.VariantName(args)`, e.g., `Color.Red`, `Payment.Card(last4: "1234")`
- `Some`, `None`, `Ok`, `Err` are exceptions: prelude makes them unqualified
- In `match` patterns, variants are written unqualified

### Trait vs Ability Distinction (spec §8.4, §8.7, §17)

This is critical and easy to get wrong:

- **Abilities** (`Eq`, `Hash`, `Ord`, `Debug`): compiler-known, derived from struct shape, **cannot be implemented manually with `impl`**
- **Traits** (`Stringable`, `Into<T>`, user-defined): require explicit `impl` blocks, support static dispatch only

When implementing the type checker:

- Auto-derive abilities for `value` and `data` per the rules in §8.6
- `Hash` requires `Eq` (enforce this constraint)
- `data` does NOT auto-derive `Ord` (use `sort_by` instead)
- `data` with any `mut` field does NOT auto-derive `Hash` (would break Set/Map invariants)
- `value` auto-derives `Ord` only for single-field types
- `Float` does NOT have `Eq` — see ADR-0002

### Error Handling Rules (spec §12)

- `?` works on **both** `Result<T, E>` and `Option<T>` — see ADR-0004
- For `Result`: enclosing function must return `Result<U, F>`, with `E: Into<F>`
- For `Option`: enclosing function must return `Option<U>`
- `Into<T> for T` is auto-provided by the compiler
- `or return` syntax does NOT exist (removed in ADR-0004)
- `or` is purely a logical operator
- To convert Option to Result for early return: `repo.find(id).ok_or(NotFound)?`
- `panic("msg")` is a regular function, NOT a macro (no `panic!()`)
- `panic` returns `Never`, which is a subtype of every type
- No exceptions, ever

### Async Rules (spec §14)

- `async fn f(...) -> T` returns `Task<T>` when called
- `.await` is postfix and binds tighter than `?`
- `fetch(id).await?` parses as `(fetch(id).await)?`
- `.await` is only legal inside `async` functions
- `spawn` only accepts function calls (NOT arbitrary expressions)
- `spawn f(args)` returns `Task<T>`
- `core.tasks.join_all` and `core.tasks.select` coordinate multiple tasks

### String Rules (spec §7.3)

- Regular strings support interpolation: `"hello #{name}"`
- Raw strings use `r"..."` and don't process escapes or interpolation
- Multi-line strings are NOT supported in v0.1
- Standard escape sequences: `\n`, `\t`, `\r`, `\\`, `\"`, `\0`, `\u{XXXX}`

### Collection Rules (spec §11)

- `items[index]` panics on out-of-bounds access
- Safe access: `items.get(index)` returns `Option<T>`
- `Map<K, V>` requires `K: Hash`

## Rust Code vs Tyra Code

The compiler is written in Rust. Rust language features (macros, panics, etc.) are unrelated to Tyra language features and should be used freely in Rust code.

### Rust Code Conventions (compiler implementation)

- Use `cargo fmt` and `cargo clippy` before every commit
- Prefer `Result<T, TyraError>` over panics for any user-facing path
- Internal invariant violations may use `unreachable!()`, `panic!`, or `todo!`, but document why. **These are Rust constructs and have no relation to Tyra's `panic` function or its lack of macros.**
- Diagnostic messages must go through `tyra-diagnostics` for i18n
- Spec references in code: `// spec §8.6: value types are immutable`

### Tyra Code Conventions (in stdlib, tests, examples)

Tyra code (`.tyra` files) follows the language specification strictly:

- Tyra has no macros (spec §3). Use `panic("msg")` as a regular function call, not `panic!("msg")`.
- Use `?` for both Result and Option propagation
- Use `and`/`or`/`not` for logical operations, not symbolic operators
- Constructor calls use qualified form (`Payment.Card(...)`)
- All other rules are defined in the spec.

## What NOT to Do

These are common mistakes that contradict Tyra's design:

- **Don't add features from Rust/Swift just because they're convenient.** If it's not in the spec, propose an RFC first under `docs/rfcs/`.
- **Don't introduce trait objects, `dyn Trait`, or any form of dynamic dispatch.** v0.1 is static dispatch only.
- **Don't add structural typing or duck typing.** Tyra is nominal.
- **Don't allow `null` to appear anywhere in error paths or default values.** Use `Option`.
- **Don't add operator overloading.** `+`, `-`, `*`, `/` are built-in for numeric types only.
- **Don't add macros, runtime reflection, or `eval`.** These are explicit non-goals (§3).
- **Don't use `&&`, `||`, `!` in Tyra code.** Use `and`, `or`, `not`.
- **Don't write `panic!("msg")` in Tyra code.** Write `panic("msg")` (function call). Macros do not exist in Tyra.
- **Don't write `or return` in Tyra code.** This syntax was removed in ADR-0004. Use `?` for both Result and Option.
- **Don't try `Float == Float`.** Float has no `Eq`. Use `float.eq()` or `float.approx_eq()`.
- **Don't put `data` with `mut` fields in `Set` or `Map` keys.** Hash auto-derivation skips them.
- **Don't write `data User { ... }` and expect `Set<User>` to work** if User has any `mut` fields. Either remove `mut` or use a different type as the key.
- **Don't add three or more generic constraints** (`<T: A + B + C>` is forbidden in v0.1, max is 2).
- **Don't add `where` clauses to generics.** Use inline constraints `<T: Constraint>`.
- **Don't add guard clauses to `match`.** Use `if/else` instead. (May be added in a future spec version via RFC.)
- **Don't pass arbitrary expressions to `spawn`.** `spawn` accepts function calls only.
- **Don't bypass spec for "ease of implementation".** If the spec is hard to implement, fix the implementation, not the spec.

## When the Spec Is Ambiguous

If you encounter a situation where the spec doesn't clearly answer the question:

1. **Stop and ask the maintainer** — do not guess
2. Document the ambiguity as a GitHub issue with the `spec-clarification` label
3. The resolution may become a spec patch (v0.1.x) or an RFC for v0.2

Never silently make a design choice that the spec doesn't endorse. The point of Tyra is predictability.

## Versioning

- **Spec versions**: tagged as `spec-v0.1.0`, `spec-v0.1.1`, `spec-v0.2.0`, ...
- **Compiler versions**: tagged as `v0.1.0`, `v0.1.1`, ...
- The compiler always declares which spec version it implements:

  ```console
  $ tyra --version
  tyra 0.1.0
  implementing language spec 0.1
  ```

Spec status is currently **Draft** (v0.1.0). Breaking changes are allowed in MINOR bumps until v1.0.

## Implementation Strategy

Build order (do not skip ahead). Each step builds on the previous ones. `tyra-diagnostics` comes first because all later crates emit errors through it. `tyra-ast` is separated from the parser because both `tyra-parser` and `tyra-types` depend on it.

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

- Unit tests in the crate
- Integration tests in `compiler/tests/`
- Conformance tests in `tests/conformance/` referencing spec sections

## Useful Spec Sections by Task

| Task | Read first |
| --- | --- |
| Implementing lexer | §5, §7.3 (including raw strings) |
| Implementing parser | §6, §8.5, §9, §10 (including else if) |
| Implementing type inference | §8.1, §8.4 (multi-constraint generics) |
| Implementing ability derivation | §8.6 (note: data with mut fields has no Hash), §8.7, §17 |
| Implementing `?` operator | §12.2 (handles both Result and Option) |
| Implementing async | §14 |
| Implementing pattern matching | §8.5, §10.3 |
| Implementing modules | §13 |
| Implementing the formatter | §20 |
| Implementing GC/runtime | §15 |
| Implementing logical operators | §10.1 (and/or/not keywords) |
| Implementing constructor calls | §8.5 (qualified form, prelude exceptions) |
| Implementing panic | §12.1 (function, not macro) |

## Examples to Reference

When implementing parser or codegen, refer to canonical examples:

- `docs/spec/ja/language-spec.md` §21 (worked examples in the spec)
- `tests/corpus/` (spec by example)
- `examples/hello/main.tyra` (minimal program)

These are the ground truth for what Tyra code looks like.

## Architecture Decision Records (ADRs)

Read these when working on the corresponding areas:

| ADR | Topic | Read when working on |
| --- | --- | --- |
| [0001](../docs/design/0001-adt-data-semantics.md) | ADT uses data semantics | type system, ADTs, codegen |
| [0002](../docs/design/0002-float-no-eq.md) | Float has no Eq | type system, ability derivation |
| [0003](../docs/design/0003-stdlib-minimal-scope.md) | Stdlib Tier 1/2 split | standard library, prelude |
| [0004](../docs/design/0004-unify-propagation-operator.md) | `?` for both Result and Option | error handling, parser |
| [0005](../docs/design/0005-multi-constraint-generics.md) | Up to 2 constraints | generics, type checker |

## Out of Scope (Do Not Implement)

These are explicit non-goals for v0.1 (spec §3, §22):

- Ownership/borrow checker
- Macros
- Operator overloading
- Trait objects, `dyn Trait`
- Runtime reflection
- Actor model (language-level)
- Foreign function interface (FFI) details
- Task cancellation
- Multi-line strings (raw strings ARE supported)
- Where clauses, 3+ constraints, associated types
- Higher-kinded types
- Guard clauses in match
- Tuple types
- Structured concurrency

If a user requests these, point them to the relevant section of §22 and offer to draft an RFC for v0.2.

## Asking for Help

When stuck or uncertain:

- For spec questions: open an issue with `spec-clarification` label
- For implementation questions: open an issue with `implementation` label
- For design proposals: draft an RFC in `docs/rfcs/`

Do not invent answers. Tyra's value depends on consistency; one wrong call can cascade.
