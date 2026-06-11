# Tyra vs Crystal

Crystal is Tyra's closest competitor. Both compile Ruby-derived syntax to native
binaries via LLVM, both have static type systems with inference, and both target
backend services and CLI tools. If you are evaluating Tyra and already know Crystal,
this page explains what is different and why.

---

## Positioning summary

| | Tyra | Crystal |
| -- | -- | -- |
| **Syntax** | Ruby-derived (`end` blocks, `#{}` interpolation) | Ruby-derived (`end` blocks, `#{}` interpolation) |
| **Type system** | Static, Hindley-Milner inference | Static, global inference |
| **Backend** | LLVM | LLVM |
| **Error model** | `Result<T, E>` + `?` propagation | Exceptions only (`raise`/`rescue`) |
| **Parallelism** | Built-in, stable | `preview_mt` — experimental since 2016 |
| **Static binary** | Linux musl (Alpine) | Alpine Linux only |
| **Macros** | None | Compile-time macros |
| **Operator overloading** | None | Supported |
| **Union types** | None (nominal ADTs only) | `String \| Int32 \| Nil` |
| **Float equality** | No `Eq` (ADR-0002, NaN-safe) | `Float64 == Float64` allowed |
| **`copy()` on value types** | Auto-provided for all `value` types | Must write manually |
| **Ability auto-derivation** | `Eq`/`Hash`/`Ord`/`Debug` with semantic rules | Manual `def ==`, `def hash` |
| **AI-gen benchmark** | 77% pass with spec injection (v0.10.0, seed-1 point estimate) | 96% pass (seed-1 point estimate; strong prior knowledge) |

**One-sentence summary**: Tyra is what Crystal would look like if designed after Go and
Rust proved that explicit error handling outperforms exceptions, and after the LLM era
proved that constrained syntax produces more reliable AI-generated code.

---

## Feature comparison

### Error handling

| | Tyra | Crystal |
| -- | -- | -- |
| Error representation | `Result<T, E>` with ADT variants | Exception classes (`Exception`, subclasses) |
| Propagation | `?` operator — compile-time, explicit | `raise`/`rescue` — implicit, invisible in types |
| Visibility in types | `fn read_config() -> Result<Config, ConfigError>` | `def read_config : Config` (raises hidden) |
| Error conversion | `?` + `Into<E>` — auto-converts error types | Must `rescue` and re-`raise` manually |
| Option | `Option<T>` with `?` | `T?` = `T \| Nil` (nil safety, but no propagation) |

Crystal's nil safety (`String?`) is genuine and prevents the `NoMethodError` problem
Ruby has. But Crystal has no `Result` type — errors are exception-based, meaning a
function's failure modes are invisible in its signature. See [deep dive 1](#1-exception-based-errors-vs-resultt-e).

### Parallelism

| | Tyra | Crystal |
| -- | -- | -- |
| Thread model | Native threads, stable | Fibers by default; threads via `preview_mt` |
| Status | Production-ready | `preview_mt` is experimental (see Crystal docs) |
| Type-safe spawn | `spawn fetch(url) : Task<Result<T, E>>` | `spawn { }` (Fibers); `Channel(T)` for typed comm |
| Join all | `tasks.join_all().await` | Manual channel receive loop |

See [deep dive 2](#2-experimental-parallelism-preview_mt).

### Static binary distribution

| | Tyra | Crystal |
| -- | -- | -- |
| Linux musl (static) | Yes — CI-verified on Alpine | Alpine Linux only |
| Linux glibc | Yes (dynamic) | Yes (dynamic) |
| macOS | Yes (dynamic) | Yes (dynamic) |
| Windows | Tracked, not yet released | Limited (no official musl equivalent) |

See [deep dive 3](#3-static-binary-alpine-only-for-crystal).

### Type system ergonomics

| | Tyra | Crystal |
| -- | -- | -- |
| Value type | `value` — always immutable | `struct` — can have mutable `property` |
| Reference type | `data` — opt-in `mut` fields | `class` — mutable by default |
| Mutation rules | `mut` field AND `mut` binding required | `property` opens mutation freely |
| Enum (ADT) | `type Color = \| Red \| Yellow \| Card(last4: String)` | `enum Color` — unit variants only, no payload |
| Hash safety | `data` with `mut` fields cannot derive `Hash` | No compile-time constraint |
| Float `==` | Compile error (E0306, ADR-0002) | Allowed (`Float64 == Float64`) |

---

## Deep dives

### 1. Exception-based errors vs `Result<T, E>`

Crystal uses `raise`/`rescue` for all error handling — the same model as Ruby and Java.
There is no `Result` type in Crystal's standard library.

```crystal
# Crystal — the function signature tells you nothing about failure modes
def read_config(path : String) : Config
  File.open(path) do |f|
    Config.from_json(f.gets_to_end)
  end
  # JSON::ParseException and File::Error can propagate silently to any caller
end
```

> **Crystal documentation**: Exception handling in Crystal is documented at
> <https://crystal-lang.org/reference/latest/syntax_and_semantics/exception_handling.html>.
> Crystal has no `Result` type equivalent in stdlib.

```tyra
# Tyra — failure modes are part of the public contract
fn read_config(path: String) -> Result<Config, ConfigError>
  let text = fs.read(path)?        # propagates Io error
  let cfg = json.parse(text)?      # propagates Json error
  Ok(cfg)
end
```

The `?` operator propagates errors up through the call stack without requiring
`rescue` blocks at each level. Combined with `Into<E>`, it converts error types
automatically (no manual `rescue ConfigError => e; raise AppError.new(e)`).

**Practical consequence**: In Tyra, every function that can fail says so in its
return type. Code review, AI-generated code, and static analysis tools can see
the complete failure contract without reading the body.

**Crystal's advantage here**: Exceptions are less verbose for simple scripts and
throwaway code. If error handling is not a priority, Crystal's model is terser.

---

### 2. Experimental parallelism (`preview_mt`)

Crystal's default concurrency model uses **Fibers** — lightweight, cooperative green
threads scheduled on a single OS thread. This is efficient for I/O-bound work but
does not use multiple CPU cores.

True multi-threading in Crystal requires the `preview_mt` flag:

```sh
crystal build --release -Dpreview_mt myapp.cr
```

> **Crystal documentation**: Multi-threading support (`preview_mt`) is documented at
> <https://crystal-lang.org/reference/latest/guides/concurrency.html>.
> The official guide marks multi-threading as experimental and notes that "not all
> standard library classes are protected against concurrent access."

This flag has existed since approximately 2016 and remains experimental over a
decade later, with known thread-safety caveats in the standard library.

Tyra's parallel runtime is built on native OS threads and is stable from the first
release. The `spawn` + `Task<T>` + `join_all` pattern is the standard way to
parallelize work:

```tyra
async fn fetch_all(urls: List<String>) -> Result<List<String>, HttpError>
  let tasks = urls.map(fn(url) spawn fetch(url))
  tasks.join_all().await?
end
```

**Crystal's advantage here**: Fibers are mature and production-proven in Crystal's
ecosystem (Lucky framework, Amber, and others use them effectively for I/O-bound
servers). If you need cooperative concurrency without multi-threading, Crystal's
fiber model is excellent.

---

### 3. Static binary — Alpine-only for Crystal

Crystal can produce a statically linked binary, but only when built on Alpine Linux
(which uses musl libc). Building a static binary on a glibc system or macOS is not
supported.

> **Crystal documentation**: Static linking in Crystal is documented at
> <https://crystal-lang.org/reference/latest/guides/static_linking.html>.
> The guide states that "the recommended way to build a statically linked Crystal
> application is to use Alpine Linux" and provides a Docker-based workflow.

This means Crystal users who want portable static binaries must maintain an Alpine
Docker build environment even for development on macOS or Debian/Ubuntu.

Tyra's situation is the same as Crystal's today: `tyra build --static` is supported
on Linux musl (Alpine) only. The `install.sh` installer distributes a static binary
built on Alpine, and the CI matrix (`release-gate.yml`) includes an Alpine job that
verifies the static build on every pull request. Static linking on Linux glibc and
macOS is tracked but not yet implemented.

```sh
# Tyra — static binary on Linux musl (Alpine)
tyra build --static myapp.ty

# Crystal — same requirement: Alpine Linux (or Docker)
docker run --rm -v $PWD:/workspace crystallang/crystal:latest \
  crystal build --release --static /workspace/myapp.cr
```

The practical difference is in distribution: Tyra's installer handles the Alpine
build for you and places a working static binary in `~/.local/bin/tyra`, so end
users on macOS or glibc Linux do not need Docker. Crystal has no equivalent
single-command installer.

**Crystal's advantage here**: Alpine-based static builds are well-understood and
widely used in Crystal's ecosystem. Both languages share the same musl constraint.

---

## Code comparisons

The Crystal implementations are in `examples/comparisons/crystal/`. The equivalent
Tyra programs live in `bench/static-corpus/` (the compiler test corpus). The full
head-to-head analysis is in [`examples/comparisons/ANALYSIS.md`](../../examples/comparisons/ANALYSIS.md).

| # | Program | Crystal |
| - | ------- | ------- |
| 01 | Hello World | [`examples/comparisons/crystal/01-hello.cr`](../../examples/comparisons/crystal/01-hello.cr) |
| 02 | Fibonacci / Pattern matching | [`examples/comparisons/crystal/02-fibonacci.cr`](../../examples/comparisons/crystal/02-fibonacci.cr) |
| 03 | Option / Result | [`examples/comparisons/crystal/03-option-result.cr`](../../examples/comparisons/crystal/03-option-result.cr) |
| 04 | HTTP handler | [`examples/comparisons/crystal/04-http-handler.cr`](../../examples/comparisons/crystal/04-http-handler.cr) |
| 05 | JSON parsing | [`examples/comparisons/crystal/05-json-parsing.cr`](../../examples/comparisons/crystal/05-json-parsing.cr) |
| 06 | CLI args | [`examples/comparisons/crystal/06-cli-args.cr`](../../examples/comparisons/crystal/06-cli-args.cr) |
| 07 | State machine | [`examples/comparisons/crystal/07-state-machine.cr`](../../examples/comparisons/crystal/07-state-machine.cr) |
| 08 | Async tasks | [`examples/comparisons/crystal/08-async-tasks.cr`](../../examples/comparisons/crystal/08-async-tasks.cr) |
| 09 | Error handling | [`examples/comparisons/crystal/09-error-handling.cr`](../../examples/comparisons/crystal/09-error-handling.cr) |
| 10 | Data modeling | [`examples/comparisons/crystal/10-data-modeling.cr`](../../examples/comparisons/crystal/10-data-modeling.cr) |

---

## When to choose Crystal

Tyra is a better fit when you need explicit error contracts, a one-command static-binary
installer, or reliable AI-generated code. Crystal is a better fit in the following cases:

**Choose Crystal if**:

- **Your team already knows Crystal** — ecosystem familiarity and library availability
  are decisive in practice. Crystal has years of production use in Lucky, Amber, and
  custom services.
- **You need macros** — Crystal's compile-time macros (`macro`, `annotation`) enable
  metaprogramming that Tyra explicitly excludes. If your design requires them, Crystal
  is the only option in this space.
- **You need union types** — Crystal's `String | Int32 | Nil` is expressive in ways
  Tyra's nominal ADTs are not. If your data is inherently heterogeneous, Crystal's
  type algebra is more natural.
- **You need `JSON::Serializable`** — Crystal's one-line struct-to-JSON mapping is
  significantly more ergonomic than anything Tyra offers today. For JSON-heavy services,
  this matters.
- **Exceptions are acceptable** — many teams work effectively with exception-based
  errors. If your team is not prioritizing explicit error types, Crystal's model is
  terser.
- **You need operator overloading** — for domain-specific numeric or collection types,
  Crystal's `def +(other)` is cleaner than Tyra's function-based approach.
- **Fiber-based concurrency is sufficient** — Crystal's event loop with Fibers is
  mature and production-proven for I/O-bound servers. If you do not need multi-core
  parallelism, Crystal's model is excellent.

---

## Summary

| Question | Recommendation |
| -------- | -------------- |
| I need explicit error types in function signatures | Tyra |
| I need a single-command installer that gives a static binary | Tyra |
| I need reliable multi-core parallelism today | Tyra |
| I need macros or operator overloading | Crystal |
| I need `JSON::Serializable` or a richer stdlib | Crystal |
| I need union types | Crystal |
| My team already knows Crystal | Crystal |
| I want AI to generate my code reliably | Tyra (77% vs 96%, both seed-1 point estimates — gap narrows as AI training on Tyra grows) |

Tyra does not claim to be better than Crystal in every dimension. Crystal has years of
ecosystem maturity that Tyra does not. The case for Tyra rests on three specific
structural differences: explicit errors in types, a one-command static-binary installer,
and AI auditability through deliberate constraints. If those differences matter for your
project, Tyra is the right choice.
