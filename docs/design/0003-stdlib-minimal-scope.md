# ADR 0003: Standard library minimal scope for v0.1

- **Status**: Accepted
- **Date**: 2026-04-15
- **Spec sections affected**: §17, §12.1, §14.4

## Context

The spec §17 lists 11 standard library modules (`core`, `string`, `collections`, `option`, `result`, `json`, `http`, `fs`, `time`, `test`, `log`) but does not define their APIs. During Phase 0a (spec by example), multiple gaps were found:

- `print` / `println` functions are undefined (SPEC_GAP F)
- Command-line argument access is undefined (SPEC_GAP G)
- `List` indexing behavior is undefined (SPEC_GAP H)
- `panic` function signature is undefined (SPEC_GAP E)
- `Task.join_all` / `select` are undefined (SPEC_GAP D)
- `http`, `fs`, `json` APIs are completely unspecified (SPEC_GAP O)

The question is: how much of the standard library must be defined before the compiler can be implemented, and how much can be deferred?

## Decision

**Split the standard library into two tiers.**

### Tier 1: Language-critical (must be defined in the language spec)

These are needed by the compiler, type checker, or core language semantics. They are defined in the language specification itself.

| Module | Contents | Why language-critical |
| -- | -- | -- |
| `core` | `Option<T>`, `Result<T, E>`, `Into<T>`, `Stringable`, `print`, `println`, `panic`, `Unit` literal `()`, `Never` type | `?` operator, `or return`, type system fundamentals |
| `core.sys` | `args()`, `env()`, `exit()` | `main` function needs access to program environment |
| `core.tasks` | `join_all`, `select` | `spawn` is meaningless without concurrent coordination |

### Tier 2: Practical (defined separately, not in the language spec)

These are important for usability but do not affect language semantics. Their APIs are defined in a separate standard library specification (`docs/spec/stdlib/`), not in the language spec.

| Module | Contents | Why separate |
| -- | -- | -- |
| `string` | `split`, `trim`, `contains`, `replace`, etc. | API design, not language semantics |
| `collections` | `List`, `Map`, `Set` methods | API design (except indexing, which is Tier 1) |
| `json` | `parse`, `stringify` | Pure library code |
| `http` | `Server`, `Client`, `Request`, `Response` | Large API surface, evolves independently |
| `fs` | `read`, `write`, `open`, `stat` | OS-dependent API |
| `time` | `now`, `Duration`, `Instant` | API design |
| `test` | `assert`, `assert_eq`, test runner | Tooling, not language semantics |
| `log` | `info`, `warn`, `error`, `debug` | API design |
| `float` | `eq`, `approx_eq`, `is_nan` | Needed due to ADR-0002 (Float has no Eq) |

### Tier 1 detailed API

#### core

```tyra
# I/O
export fn print<T: Debug>(_ value: T) -> Unit
export fn println<T: Debug>(_ value: T) -> Unit
export fn eprint<T: Debug>(_ value: T) -> Unit
export fn eprintln<T: Debug>(_ value: T) -> Unit

# Program control
export fn panic(_ message: String) -> Never

# Unit literal
# () is the Unit literal, available without import
```

#### core.sys

```tyra
export fn args() -> List<String>
export fn env(_ key: String) -> Option<String>
export fn exit(_ code: Int) -> Never
```

#### core.tasks

```tyra
export fn join_all<T>(_ tasks: List<Task<T>>) -> Task<List<T>>
export fn select<T>(_ tasks: List<Task<T>>) -> Task<T>
```

Note: `join2<A, B>` (joining two tasks of different types) is deferred because it requires tuple types, which are not in v0.1. Users can work around this by wrapping results in an ADT.

### Collections indexing (Tier 1)

`List<T>` indexing is language-critical because `items[index]` is core syntax:

```tyra
# items[index] panics if index is out of bounds
let x = items[0]

# Safe access returns Option<T>
let y = items.get(0)
```

This follows the Rust convention: `[]` panics, `.get()` returns `Option`. The rationale is that wrapping every index access in `Option` handling is too verbose for a practical language.

### String escapes (Tier 1)

String escape sequences are part of lexical rules and must be in the language spec:

```txt
\n    newline
\t    tab
\r    carriage return
\\    backslash
\"    double quote
\0    null byte
\u{XXXX}  Unicode code point (1-6 hex digits)
```

## Consequences

### What this enables

- The compiler can be implemented with only Tier 1 APIs defined
- Tier 2 APIs can be designed iteratively during or after compiler development
- The language spec stays focused on semantics, not library design
- Third-party libraries can fill gaps before Tier 2 is complete

### What this defers

- `http`, `fs`, `json` API design (potentially 6+ months of work)
- Full `collections` API (`List.map`, `List.filter`, `List.reduce`, etc.)
- Test framework design
- Logging framework design

### Documentation structure

```txt
docs/
├── spec/
│   ├── ja/
│   │   └── language-spec.md          # includes Tier 1 APIs
│   └── en/
│       └── language-spec.md
└── stdlib/                            # Tier 2 APIs (separate docs)
    ├── string.md
    ├── collections.md
    ├── json.md
    ├── http.md
    ├── fs.md
    ├── time.md
    ├── test.md
    ├── log.md
    └── float.md
```

### Never type

The introduction of `panic` and `exit` requires a `Never` type (a type with no values, indicating that a function does not return). This must be added to spec §7.2:

```tyra
fn panic(_ message: String) -> Never
fn exit(_ code: Int) -> Never
```

`Never` is a bottom type: it is a subtype of all types, allowing:

```tyra
let x: Int = if condition
  42
else
  panic("unreachable")  # Never coerces to Int
end
```

## Alternatives considered

### A. Define all APIs in the language spec

Define `http.Server`, `fs.read`, `json.parse`, etc. in §17 of the language spec.

Rejected because:

- The language spec would grow from 1000 to 3000+ lines
- API design requires implementation feedback (especially `http` and `fs`)
- Spec changes would block on library design discussions
- Mixing language semantics with library APIs makes the spec harder to read

### B. Define no APIs (not even print)

Ship the spec with zero function signatures. Let the standard library evolve independently.

Rejected because:

- `print("hello")` is the first thing anyone writes
- `panic` is required by the language semantics (§12.1)
- `Option`/`Result` constructors are used in spec examples
- The compiler needs to know about `Into<T>` for `?` operator
- Without Tier 1, the spec is incomplete and untestable

### C. Define all APIs but mark Tier 2 as "unstable"

Include all APIs in the spec with a stability annotation.

Rejected because:

- "Unstable API in the spec" is a contradiction
- The spec should only contain stable, decided features
- Unstable APIs belong in a separate document or RFC

## References

- Spec §17 (Standard library)
- Spec §12.1 (Error handling principles — panic)
- Spec §14.4 (spawn)
- Phase 0a: SPEC_GAP D, E, F, G, H, O
- Go standard library scope: <https://pkg.go.dev/std>
- Rust standard library scope: <https://doc.rust-lang.org/std/>
