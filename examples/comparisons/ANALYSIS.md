# Phase 0b: Tyra vs Gleam vs V vs Ruby vs Crystal — Comparative Analysis

## Overview

The same 10 programs were written in Tyra, Gleam, V, Ruby, and Crystal to identify
whether Tyra offers capabilities or ergonomic advantages not available in competitors.

Crystal is the most critical comparison. It occupies the same niche — "Ruby syntax
with static types, compiled to native binary via LLVM" — so we must identify what
Tyra offers that Crystal does not.

## Feature-by-feature comparison

### 1. Hello World (01)

| | Tyra | Gleam | V | Ruby | Crystal |
| -- | -- | -- | -- | -- | -- |
| Lines | 4 | 5 | 4 | 1 | 1 |
| Boilerplate | `fn main` | `import gleam/io` | `fn main` | none | none |
| Compiled | Yes (LLVM) | Yes (BEAM) | Yes | No (interp) | Yes (LLVM) |

**Verdict**: Ruby and Crystal are identical — no main function needed. Tyra requires
`fn main`. This is intentional (explicit entry point for AI parsing), but adds ceremony.

### 2. Fibonacci / Pattern Matching (02)

| | Tyra | Gleam | V | Ruby | Crystal |
| -- | -- | -- | -- | -- | -- |
| Pattern match | `match/when/end` | `case { }` | `match { }` | `case/when/end` | `case/when/end` |
| String interp | `"#{expr}"` | `<>` concat | `'${expr}'` | `"#{expr}"` | `"#{expr}"` |
| Typing | Static `Int` | Static `Int` | Static `int` | Dynamic | Static `Int32` |

**Verdict**: Tyra, Ruby, and Crystal share `case/when/end` + `#{}`. Crystal adds
static types like Tyra. At this level, **Tyra and Crystal are nearly identical**.

### 3. Option/Result (03)

| | Tyra | Gleam | V | Ruby | Crystal |
| -- | -- | -- | -- | -- | -- |
| Absence | `Option<T>` | `Option(T)` | `?T` | `nil` | `T?` = `T \| Nil` |
| Error | `Result<T, E>` generic | `Result(T, E)` | `!T` | Exceptions | Exceptions |
| Propagation | `?` operator | `use <- result.try` | `or { }` | `raise/rescue` | `raise/rescue` |
| Nil safety | Compile-time | Compile-time | Compile-time | None | **Compile-time** |
| Typed errors | ADT variants | Custom types | `IError` | Exception classes | Exception classes |

**Critical comparison — Tyra vs Crystal**:

- **Crystal has nil safety** — `String?` is `String | Nil`, and the compiler forces
  nil checks before use. This eliminates Ruby's `NoMethodError` problem.
- **Crystal has NO Result type** — errors use exceptions only. `raise`/`rescue` is
  the only mechanism. Error paths are NOT visible in type signatures.
- **Tyra's `?` operator has no Crystal equivalent** — Crystal relies on exceptions,
  which can propagate silently. Tyra makes every error explicit in the return type.
- **Tyra's `Into` trait enables automatic error conversion** — Crystal has no analog;
  you must catch and re-raise manually.

**Tyra advantage over Crystal**: `Result<T, E>` makes errors explicit; `?` + `Into`
gives ergonomic propagation without losing type information.

### 4. HTTP Handler (04)

| | Tyra | Gleam | V | Ruby | Crystal |
| -- | -- | -- | -- | -- | -- |
| Framework | `http.server` | wisp | vweb | Sinatra | `HTTP::Server` (stdlib) |
| Async model | `async fn` + `.await?` | BEAM processes | Coroutines | Thread pool | Fibers (evented) |
| Handler type | `async fn(Req) -> Result<Resp, E>` | `fn(Req) -> Resp` | Method+attr | Block | Block |
| Error in type | Yes | No | No | No | No |

**Critical comparison — Tyra vs Crystal**:

- **Crystal's HTTP::Server uses a block** — `do |context| ... end`. No typed error
  in the handler signature. Errors silently crash the handler or are swallowed.
- **Crystal's Fiber model is implicit** — the runtime handles concurrency internally.
  You don't write `async`/`await`. Simpler for some, but hides suspension points.
- **Tyra forces `Result<Response, E>`** — handler errors are part of the contract.

**Tyra advantage**: Explicit async + typed handler errors.
**Crystal advantage**: Simpler code for the same functionality; stdlib HTTP server
is production-ready with no additional imports.

### 5. JSON Parsing (05)

| | Tyra | Gleam | V | Ruby | Crystal |
| -- | -- | -- | -- | -- | -- |
| Parse | `json.parse(input)?` | `json.decode` | `json.decode(T)` | `JSON.parse` | `JSON.parse` |
| Error types | ADT + `Into` | `map_error` | String | Exception class | Exception class |
| Struct mapping | TBD (Tier 2) | `dynamic` decoders | Built-in | Manual | `JSON::Serializable` |

**Critical comparison — Tyra vs Crystal**:

- **Crystal has `JSON::Serializable`** — `include JSON::Serializable` on a struct
  gives automatic JSON deserialization. Tyra's json API is Tier 2 (TBD).
- **Crystal's JSON errors are exceptions** — `JSON::ParseException` must be caught
  with `rescue`. No way to express "this function can fail with JsonError" in the type.
- **Tyra's ADT + match** — nested error matching (`when Err(Json(inner: MissingKey(...)))`)
  gives exhaustive, type-safe error handling that Crystal can't match.

**Tyra advantage**: Typed error chain with exhaustive match.
**Crystal advantage**: `JSON::Serializable` is more ergonomic than anything Tyra has yet.

### 6. CLI Args (06)

| | Tyra | Gleam | V | Ruby | Crystal |
| -- | -- | -- | -- | -- | -- |
| Args access | `sys.args()` | `argv.load()` | `os.args` | `ARGV` | `ARGV` |
| Safe access | `.get(n).ok_or()?` | `list.at \|> replace_error` | `args[n]` | `ARGV[n]` (nil) | `ARGV[n]?` (nil) |
| Parse int | `parse::<Int>(text)` | `int.parse` | `text.int()` | `Integer(text)` | `text.to_i?` |

**Critical comparison — Tyra vs Crystal**:

- **Crystal's `ARGV[n]?` returns `String?`** — nil-safe, compiler forces check.
  Combined with `to_i?` (returns `Int32?`), Crystal handles the happy path well.
- **But Crystal has no typed error** — `abort "error"` is the escape hatch.
  Tyra's `Result<Unit, CliError>` with ADT variants gives structured error reporting.

**Tyra advantage**: Typed CLI errors enable programmatic error handling.
**Crystal advantage**: Terser for simple cases (`ARGV[0]? || abort "msg"`).

### 7. State Machine (07)

| | Tyra | Gleam | V | Ruby | Crystal |
| -- | -- | -- | -- | -- | -- |
| ADT | `type Color = \| Red ...` | `pub type Color { Red }` | `enum` | Symbols | `enum Color` |
| Value type | `value TrafficLight` | Custom type | `struct` | `Struct.new` | `struct` |
| Copy | `.copy(color:)` | `..record` spread | New literal | `dup.tap` | Manual `copy` method |
| Exhaustiveness | Compile-time | Compile-time | Compile-time | None | **Compile-time** |

**Critical comparison — Tyra vs Crystal**:

- **Crystal has `enum`** — but enums can't carry data (unlike Tyra's ADTs).
  Crystal's `enum Color` is equivalent to Tyra's `type Color = | Red | Yellow | Green`
  for unit variants, but can't do `| Card(last4: String)`.
- **Crystal's `case` on enum is exhaustive** — the compiler warns on missing cases.
- **Crystal's `struct` is a value type** — similar to Tyra's `value`. Crystal also
  has `class` for reference types. **This is the closest analog to `value`/`data`.**
- **Crystal has no built-in `copy()`** — you must write a manual method or use
  `Struct#clone` (shallow copy only, no field-update syntax).

**Tyra advantage**: ADTs with data-carrying variants; built-in `copy()`.
**Crystal advantage**: Mature enum with methods, flags, and exhaustive matching.

### 8. Async Tasks (08)

| | Tyra | Gleam | V | Ruby | Crystal |
| -- | -- | -- | -- | -- | -- |
| Spawn | `spawn fetch(url)` | `process.start` | `spawn fn(){}` | `Thread.new` | `spawn { }` |
| Join all | `tasks.join_all().await` | `list.try_map(receive)` | Channel loop | `map(&:value)` | Channel loop |
| Type safety | `Task<Result<T, E>>` | `Subject(Result(T, E))` | `chan !T` | None | `Channel(T)` |
| Async/await | `.await?` postfix | Process receive | Channel receive | Blocking | **None** |

**Critical comparison — Tyra vs Crystal**:

- **Crystal uses Fibers + Channels** — Go-style `spawn` + `Channel(T).new`.
  Very similar to Go, but no async/await.
- **Crystal's channel is typed** — `Channel(String | FetchError).new`. Type-safe
  but union types (not Result) means error handling uses `case` on the raw value.
- **Crystal has no `join_all`** — must manually receive from each channel.
- **Tyra's `spawn` + `join_all` + `.await?`** — linear, type-safe, error-propagating.

**Tyra advantage**: `join_all` + `?` error propagation is significantly cleaner.
**Crystal advantage**: Fibers are mature and production-proven (used in Lucky, Amber).

### 9. Error Handling (09)

| | Tyra | Gleam | V | Ruby | Crystal |
| -- | -- | -- | -- | -- | -- |
| Cleanup | `defer` | BEAM GC | `defer` | `ensure` | `ensure` |
| Error chain | `? with Into` | `use <- try \|> map_error` | `or { error() }` | `rescue` | `rescue` |
| Panic | `panic("msg")` | `panic as "msg"` | `panic()` | `raise` | `raise` |
| Bool ops | `and`, `or`, `not` | `&&`, `\|\|`, `!` | `&&`, `\|\|`, `!` | Both sets | `&&`, `\|\|`, `!` |

**Critical comparison — Tyra vs Crystal**:

- **Crystal uses `ensure`** (like Ruby) — equivalent to `defer`. Both work.
- **Crystal's error propagation is exception-based** — `rescue ConfigError` catches,
  but the caller has no way to know a function raises without reading the code.
  Crystal has no `throws` annotation or `Result` return type.
- **Tyra's `Result<T, E>` is self-documenting** — `fn read_config -> Result<String, ConfigError>`
  tells you exactly what can go wrong.

**Tyra advantage**: Self-documenting error contracts via return types.

### 10. Data Modeling (10)

| | Tyra | Gleam | V | Ruby | Crystal |
| -- | -- | -- | -- | -- | -- |
| Value type | `value` | All immutable | `struct` | `Struct.new` | **`struct`** |
| Reference type | `data` | All immutable | `struct` | `class` | **`class`** |
| Mutation control | `mut` field + `mut` binding | N/A | `mut` param | `attr_accessor` | `property` setter |
| Traits | `trait` + `impl` | Functions | `interface` | `include` modules | **`include` modules** |
| Ability derivation | Auto Eq/Hash/Ord/Debug | Structural `==` | Structural `==` | Manual | **Manual** |
| Float equality | **No Eq** (ADR-0002) | `==` works | `==` works | `==` works | **`==` works** |
| Copy with update | `copy(field: val)` | `..record` spread | `...struct` | `dup.tap` | **Manual** |

**Critical comparison — Tyra vs Crystal**:

- **Crystal has `struct` (value) and `class` (reference)** — the closest analog to
  Tyra's `value`/`data`. Crystal's `struct` is stack-allocated and copied on assignment.
  Crystal's `class` is heap-allocated and GC-managed.
- **BUT Crystal doesn't enforce immutability on struct** — a Crystal `struct` can
  have `property` (mutable setter). Tyra's `value` fields are always immutable.
- **Crystal has no ability auto-derivation** — `==` and `hash` must be manually
  implemented or rely on `Struct#==` (which compares all fields, including Float).
  There is no compile-time rule like "data with mut fields can't be hashed".
- **Crystal allows `Float == Float`** — same NaN issue as Ruby. No ADR-0002 equivalent.
- **Crystal has no `copy()` with named field updates** — you must write a manual method.
  Tyra auto-provides `copy()` for all `value` types.
- **Crystal's `property` on class** allows mutation without the caller's binding being
  `mut`. Tyra requires `mut` binding + `mut` field (double opt-in).

**Tyra advantage**: Enforced immutability on `value`, auto-derived abilities with
semantic rules, `copy()`, Float-no-Eq, double opt-in mutation.
**Crystal advantage**: Mature `struct`/`class` with full method dispatch, generics,
macros, and a production-proven standard library.

---

## Head-to-head: Tyra vs Crystal

Crystal is the most direct competitor, sharing:

- Ruby-derived syntax (`end` blocks, `#{}` interpolation, `case/when`)
- Static type system with inference
- Compiled to native via LLVM
- Value types (`struct`) and reference types (`class`)
- Nil safety at compile time

### What Tyra has that Crystal doesn't

| Feature | Tyra | Crystal |
| -- | -- | -- |
| **Result<T, E> in stdlib** | First-class, with `?` propagation | None — exceptions only |
| **`?` on Option + Result** | Unified propagation | N/A |
| **`Into` auto-conversion** | `?` converts error types via `Into` | N/A |
| **Enforced value immutability** | `value` fields are always immutable | `struct` can have mutable `property` |
| **Ability auto-derivation** | Eq/Hash/Ord/Debug with semantic rules | Manual `def ==`, `def hash` |
| **Hash safety for mutable types** | `data` with `mut` fields can't derive Hash | No compile-time constraint |
| **Float has no Eq** | ADR-0002 prevents NaN bugs | `Float64#==` exists |
| **`copy()` with named fields** | Auto-provided for `value` types | Must write manually |
| **Double opt-in mutation** | `mut` field + `mut` binding required | `property` opens mutation freely |
| **`and`/`or`/`not` only** | No `&&`/`\|\|`/`!` ambiguity | Both (Crystal only has `&&`/`\|\|`) |
| **Explicit async/await** | `async fn` + `.await?` | Implicit Fibers |
| **`join_all`/`select`** | `core.tasks.join_all` | Manual channel loop |
| **ADT with data-carrying variants** | `type Payment = \| Card(last4: String)` | `enum` can't carry named fields |
| **AI-friendly design** | No macros, no overloading, no implicit | Macros, overloading, duck typing |

### What Crystal has that Tyra doesn't

| Feature | Crystal | Tyra |
| -- | -- | -- |
| **Macros** | Compile-time metaprogramming | Explicitly excluded (§3) |
| **Operator overloading** | `def +(other)` | Excluded |
| **Union types** | `String \| Int32 \| Nil` | Nominal types only |
| **`JSON::Serializable`** | Auto JSON mapping | Tier 2 (TBD) |
| **Mature stdlib** | HTTP, JSON, crypto, DB, etc. | Tier 2 not yet specified |
| **Production track record** | Lucky, Amber, real-world apps | Pre-alpha |
| **Method dispatch on structs** | `struct Point; def distance(...); end` | Functions only (no methods on value) |
| **Generics on structs** | `struct Wrapper(T)` | Supported but no method dispatch |
| **Fiber scheduler** | Production-proven event loop | TBD |
| **`include`/`extend` modules** | Mix-in composition | `trait` + `impl` (different model) |

### The honest question: "Why not just use Crystal?"

Crystal already provides Ruby syntax + static types + LLVM compilation.
Tyra's answer must be one of these:

1. **Result<T, E> + `?` is fundamental, not a nice-to-have.**
   Crystal's exception-only model means error paths are invisible in types.
   For backend services (Tyra's target), knowing exactly what can fail is critical.
   This is the same argument Rust and Go make against exception-based languages.

2. **AI-friendliness requires constraints Crystal won't add.**
   Crystal has macros, operator overloading, duck typing via `responds_to?`, and
   multiple ways to express the same thing. These are features Crystal's community
   values. Tyra deliberately removes them for AI parseability and predictability.

3. **The `value`/`data` + ability system prevents real bugs.**
   Crystal allows mutable structs in Sets, Float == Float, and mutation through any
   reference. These are sources of real bugs that Tyra prevents at compile time.

4. **Unified toolchain (fmt, test, mod) like Go.**
   Crystal has `crystal tool format` but the rest of the toolchain is fragmented
   (shards for package management, spec for testing, etc.). Tyra aims for Go-level
   operational simplicity.

---

## Summary: What can Tyra do that no competitor can?

### Unique to Tyra (not in Gleam, V, Ruby, or Crystal)

1. **`value` / `data` with enforced semantics** — Crystal has `struct`/`class` but
   doesn't enforce immutability on structs or prevent mutable-field types from being
   hashed. Tyra's rules are stricter and prevent real bugs.

2. **Ability auto-derivation with semantic rules** — no competitor auto-derives
   Eq/Hash/Ord/Debug with rules like "mut fields block Hash" or "Ord only for
   single-field values".

3. **Float has no Eq** — all competitors allow `float == float`.

4. **`?` on both Option AND Result with `Into` conversion** — Crystal has neither
   Result nor `?`. Gleam has no `?`. V has no typed errors.

5. **AI-friendly by design** — no macros, no overloading, no duck typing, no
   implicit conversions, no multiple syntax paths. Crystal, Ruby, and V all have
   features that create ambiguity for AI code generation.

### The strongest differentiators (for marketing)

**vs Crystal** (closest competitor):

- "Crystal but with Result<T, E> instead of exceptions"
- "Crystal but your Float comparisons can't silently fail"
- "Crystal but the compiler prevents mutable data in HashSets"

**vs Ruby** (syntax ancestor):

- "Ruby's readability with compile-time null safety"
- "Ruby but every error is in the type signature"

**vs Go** (operational model):

- "Go's simplicity with Ruby's readability and real generics"

---

## Conclusion

**Tyra's strongest case is against Crystal.** Crystal is the nearest competitor, and
the differentiators are genuine:

1. `Result<T, E>` + `?` (Crystal has only exceptions)
2. Enforced `value` immutability + ability derivation (Crystal's struct is too permissive)
3. Float-no-Eq (Crystal allows it)
4. AI-friendly constraints (Crystal has macros/overloading)

**The risk**: Crystal already exists, has a community, and solves 90% of what Tyra
aims to solve. Tyra's 10% improvement must be compelling enough to justify the cost
of a new ecosystem. The "Result over exceptions" argument is the strongest —
it's the same argument that made Go and Rust successful against C++/Java.

**Recommendation**: If Tyra proceeds, position it explicitly against Crystal:
"What Crystal would look like if designed after Go and Rust proved that explicit
error handling is better than exceptions."
