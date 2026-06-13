# AGENTS.md

This file provides guidance to AI coding assistants (Claude Code, Codex, Cursor, Aider, Continue, Cline, etc.) when working with the Tyra language project.

## Project Overview

Tyra is a statically-typed, AI-friendly programming language designed for backend services, CLI tools, and business applications. It compiles to native binaries via LLVM.

**Current stage**: Pre-1.0. Implementing Language Spec v0.11.

**Core design principles** (from spec §2):

1. Explicitness over implicitness — no null, no truthy/falsy, no implicit conversions
2. Single-interpretation syntax — same input must produce same AST
3. AI-friendly — design choices favor LLM completion stability over expressive shortcuts
4. Practical static typing — Option/Result are first-class
5. Simple operations — unified toolchain, single binary output

When in doubt:

- For **what Tyra is** (positioning, roadmap, "should we add this feature?"), refer to **`docs/strategy.md`**
- For **how Tyra works** (syntax, types, semantics), refer to **`docs/spec/ja/language-spec.md`**
- For **why a past decision was made**, refer to **`docs/design/`** (ADRs)

The specification is the source of truth for language semantics. The strategy document is the source of truth for project direction.

## Project Positioning

Tyra exists in a 5-layer competitive landscape. Understanding this landscape is essential for evaluating proposed features, framing documentation, and avoiding off-mission contributions.

This section is a **summary**. For the complete strategic analysis (acquisition strategy, success modes, risk analysis, decision framework, roadmap), see **`docs/strategy.md`**. AI assistants making non-trivial decisions should consult that document.

### Layer 1: Direct design competitor — Crystal

Crystal occupies the same surface position as Tyra: Ruby-like syntax, static typing with inference, LLVM-native compilation, GC. **This is the closest existing language to Tyra.**

Tyra differentiates from Crystal by:

- **Result/Option + `?`** instead of exceptions (Crystal kept Ruby's exception model)
- **No macros** (Crystal has powerful compile-time macros)
- **No operator overloading** (Crystal allows it)
- **No runtime reflection** (Crystal has duck typing via `responds_to?`)
- **Stricter `value`/`data` distinction** (Crystal's `struct` permits mutable `property`)
- **Float has no Eq** (Crystal allows `Float == Float`, NaN bugs slip through)
- **Ability auto-derivation with semantic rules** (Crystal requires manual `==`, `hash`)

The selling line: "What Crystal would look like if designed after Rust and Go proved that explicit error handling is better than exceptions, and after the LLM era proved that constrained syntax is better than expressive freedom."

### Layer 2: Strategic benchmark — Go

Go is the gold standard for **operational simplicity**: `gofmt`, `go test`, `go mod`, single binary output, fast compilation, unified toolchain. The Tyra spec explicitly targets Go-style operations (§2.4, §18).

**Go is not a market to displace.** Go users are largely satisfied with Go's tradeoffs (including `if err != nil` chains and `nil` semantics). Attempting to "convert Go users" is not a viable initial strategy.

Instead, Tyra **borrows Go as a quality benchmark**:

- "Can Tyra build/test/format/deploy as simply as Go?"
- "Can Tyra produce a single static binary as easily as Go?"
- "Is Tyra's standard toolchain as integrated as Go's?"

When in doubt about toolchain or operational design, ask: "What would Go do?" Then meet or exceed that bar.

### Layer 3: Philosophical competitor — Gleam

Gleam shares Tyra's commitment to **type safety, Result-based error handling, no null, AI-friendly determinism**. Despite running on different platforms (BEAM/JS for Gleam, LLVM-native for Tyra), the **message space overlaps significantly**: developers seeking "a modern, type-safe, predictable language" will compare both.

Tyra differentiates from Gleam by:

- **Imperative style** with `mut`, `value`/`data`, explicit binding semantics (Gleam is functional)
- **Native single-binary deployment** (Gleam targets BEAM/JS runtimes)
- **Ruby/Swift/Go-influenced surface syntax** (Gleam is more ML-influenced)

Do not dismiss Gleam as "different domain." A developer choosing between Tyra and Gleam will weigh philosophy alongside platform.

### Layer 4: Message-space competitor — V

V markets itself with: "simple, fast, safe, compiled, no null, Option/Result, immutable by default, native binary." **This phrasing overlaps almost completely with Tyra's elevator pitch.**

Tyra differentiates from V by:

- **Narrower semantics** — fewer escape hatches (no `unsafe`, no `autofree`, no compile-time reflection of V's flexibility)
- **Stricter convention fixity** — formatter-enforced layout, fixed import form, no shadowing
- **Argument labels** (Swift-style) for API self-documentation
- **Stricter error handling** (Result + `?` + `Into` vs V's `or { }` blocks)
- **Stricter `value`/`data` semantics** (V has no equivalent enforcement)

Do not compete with V on "simpler" or "faster" — those are V's selling points and V has years of head start. Compete on **predictability and team-deployable convention fixity**.

### Layer 5: Syntactic ancestor — Ruby

Tyra borrows surface syntax from Ruby (`end` blocks, `#{}` interpolation, `match/when`). **This creates expectation risk.** Ruby developers approaching Tyra may expect:

- Dynamic dispatch and duck typing
- Metaprogramming (`method_missing`, `define_method`, `instance_eval`)
- Implicit receivers and `foo bar` call style
- DSL-friendliness (Rails-style)
- Rapid prototyping with minimal ceremony

**Tyra rejects all of these.** Ruby readability is borrowed; Ruby flexibility is not.

When writing documentation that mentions Ruby influence, **always clarify what is NOT inherited** to manage expectations. Ruby users who try Tyra expecting "compiled Ruby" will be disappointed; users who understand "Ruby-readable but stricter" will not.

### Tyra in one sentence

Tyra is a Ruby-readable native language that strips Crystal's metaprogramming, mirrors Go's operational simplicity, and constrains itself more strictly than Gleam or V — designed to be auditable by both humans and AI.

### Implications for design decisions

When evaluating a proposed feature, ask:

1. **Does Crystal have it?** If yes, does Tyra's reason for excluding/changing it follow from "fewer escape hatches" or "Result over exceptions"?
2. **Would Go reject it as too clever?** If yes, Tyra should probably also reject it.
3. **Does Gleam have a competing approach?** If yes, why is Tyra's better in the imperative/native context?
4. **Does V have it as a selling point?** If yes, Tyra needs a different story (not "simpler" or "faster" — V already owns those).
5. **Would Ruby users assume it works like Ruby?** If yes, document the difference loudly.

When in doubt, prefer **less power, more determinism**. Tyra wins by being more predictable, not by being more powerful.

For larger architectural decisions (new language features, major API changes, deprecating existing functionality), apply the full 5-step decision framework in `docs/strategy.md` §9, which adds checks against the spec, ADRs, the three axes of victory (AI auditability, Crystal's structural weaknesses, Go-level operational simplicity), and the explicit "battles to avoid" list.

## Repository Structure

```text
tyra/
├── docs/strategy.md               # Strategic positioning, roadmap, decision framework
├── docs/spec/ja/language-spec.md  # Language specification (Japanese, authoritative)
├── docs/spec/en/                  # English translation (may lag)
├── docs/design/                   # ADRs (Architecture Decision Records)
├── docs/rfcs/                     # Future change proposals
├── examples/comparisons/           # Phase 0b: same programs in Gleam/V/Ruby/Crystal
│   ├── ANALYSIS.md                # Cross-language comparative analysis
│   ├── gleam/, v/, ruby/, crystal/
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
├── bench/static-corpus/           # Spec compliance corpus (positive + bad/ negative cases)
└── examples/                      # User-facing sample programs and spec-by-example
```

## Critical Reading Order

Before making any non-trivial change, AI assistants should read these in order:

1. `docs/spec/ja/language-spec.md` — entire spec, especially §8 (type system) and §12 (error handling)
2. `docs/design/` — past design decisions and their rationale (ADRs 0001-0006)
3. The relevant crate's `README.md` if present
4. Existing corpus programs in `bench/static-corpus/` (and crate tests) for the area being modified

For strategic decisions (new features, competitive positioning, roadmap changes), start with **`docs/strategy.md`** and the "Project Positioning" section above (in this file) before reading the spec.

## Language and Communication

- **Specification document**: Japanese (authoritative)
- **Code comments**: English
- **Identifiers and APIs**: English (ASCII only, no Unicode identifiers)
- **Error messages**: English by default, i18n via `TYRA_LANG=ja`
- **Commit messages**: English preferred
- **Issues/PRs**: English preferred, Japanese acceptable
- **Conversation with the maintainer**: Japanese

When generating code, comments, and identifiers must be in English. When discussing design or explaining decisions in chat, Japanese is fine.

## Platform support

See **README.md § Platform support** for the canonical table (glibc dynamic / musl static / macOS dynamic / Windows deferred). That section is the single source of truth; do not duplicate platform or link-mode descriptions in this file.

Key constraint relevant to implementation work: `tyra build --static` is only supported when the host clang targets musl (verified at runtime via `clang -print-target-triple`). glibc static linking is unsupported.

## Build and Test Commands

```bash
# Build the compiler and runtime (workspace root: tyra/)
cargo build

# Run all tests (compiler + runtime)
cargo test

# Run conformance tests only
cargo test --test conformance

# Run a single Tyra program (after build)
./target/debug/tyra run examples/01-hello.ty

# Format check (when formatter exists)
./target/debug/tyra fmt --check stdlib/

# Build release binary
cargo build --release
```

When implementing a new feature, **always add a corresponding corpus program** in `bench/static-corpus/` (error cases go in `bad/` with the expected `Exxxx` code as the filename prefix) plus crate-level unit tests, with a comment referencing the spec section.

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

Tyra code (`.ty` files) follows the language specification strictly:

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

### Don't (positioning and messaging)

These are mistakes in how Tyra is described or compared to other languages:

- **Don't market Tyra as "simpler than V" or "faster than Crystal."** V already owns "simple"; Crystal already owns "fast Ruby." Compete on predictability and AI-auditability instead.
- **Don't pitch Tyra as a "Ruby successor" or "compiled Ruby."** Ruby developers will expect dynamic flexibility, metaprogramming, and DSL-friendliness — none of which Tyra provides. Always clarify "Ruby-readable but stricter."
- **Don't claim Tyra "displaces Go."** Go is a strategic benchmark, not a market to capture. Borrow Go's operational standards as a quality bar, not as a competitive target.
- **Don't ignore Crystal in comparisons.** Crystal is the closest existing language to Tyra. Any comparison that omits Crystal is incomplete and will be seen as evasive by informed readers.
- **Don't add "AI-friendly" as a standalone selling point.** Many languages (Gleam, V) are also "AI-friendly" in some sense. Tyra's distinction is specifically AI-auditability via fixed conventions and removed escape hatches.
- **Don't add features that Crystal has just because Crystal has them** (e.g., macros, operator overloading, `responds_to?`). The point of Tyra is to remove these.

## When the Spec Is Ambiguous

If you encounter a situation where the spec doesn't clearly answer the question:

1. **Stop and ask the maintainer** — do not guess
2. If the ambiguity is about whether a feature belongs in Tyra (rather than how it should work), consult `docs/strategy.md` §9 (Decision Framework)
3. Document the ambiguity as a GitHub issue with the `spec-clarification` label
4. The resolution may become a spec patch (e.g. v0.4.x) or an RFC for the next minor version

Never silently make a design choice that the spec doesn't endorse. The point of Tyra is predictability.

## Versioning

- **Spec versions**: tagged as `spec-v0.1.0`, `spec-v0.2.0`, `spec-v0.3.0`, `spec-v0.4.0`, ...
- **Compiler versions**: tagged as `v0.1.0`, `v0.2.0`, `v0.3.0`, `v0.4.0`, ...
- The compiler always declares which spec version it implements:

  ```console
  $ tyra --version
  tyra 0.11.0
  implementing language spec 0.11
  ```

Spec status is currently **Stable** (v0.11). Breaking changes are allowed in MINOR bumps until v1.0.

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
- Corpus programs in `bench/static-corpus/` referencing spec sections

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
- `examples/` (spec by example programs)
- `examples/hello/main.ty` (minimal program)

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
| [0006](../docs/design/0006-top-level-expressions.md) | Top-level expressions as implicit main | parser, driver, entry point |
| [0007](../docs/design/0007-boehm-gc-reference-impl.md) | Boehm GC as v0.1 reference collector | codegen, driver, runtime |

### Entry-point style guidance (ADR-0006)

Tyra allows two styles for entry-point files. Use the right one for the task:

- **Top-level style** — use for hello world, simple scripts, and examples that don't need error propagation or async
- **Explicit `fn main`** — use for production apps, error-propagating entry points (`?`), async entry points (`.await`)
- **Never mix both** in one file (compile error)
- **When in doubt, use explicit `fn main`** — it is always valid

Top-level executable statements are desugared to `fn main() -> Unit`. The following are **not allowed** at the top level: `?`, `.await`, `return`. If any of these are needed, use explicit `fn main`.

Declarations (`fn`, `type`, `value`, `data`, `trait`, `impl`) may coexist with top-level executable statements — they remain outside the implicit main. Forward references to declarations are allowed.

When generating Tyra code:

- Simple prompt ("write hello world") → top-level style
- Anything involving `Result`, `async`, or production use → explicit `fn main`

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
