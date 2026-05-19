# Getting Started with Tyra

Welcome to Tyra — a statically-typed language designed for backend services, CLI tools, and business applications. Tyra compiles to native binaries via LLVM and emphasizes explicitness, predictability, and practical error handling.

> **v0.3.0** — project lifecycle, `tyra new`, `tyra mod`, `tyra bench ai-gen`, `tyra test --filter`, `tyra fmt` line wrapping. Breaking changes may occur before v1.0.

## Table of Contents

1. [Installation](01-installation.md) — Build from source and set up your environment
2. [Hello, World](02-hello-world.md) — Variables, types, functions, and string interpolation
3. [Control Flow](03-control-flow.md) — `if/else`, `for`, `while`, `match`
4. [Collections](04-collections.md) — `List<Int>`, string utilities, and `Map<String, Int>`
5. [Error Handling](05-error-handling.md) — `Option<T>`, `Result<T, E>`, and the `?` operator
6. [Types and ADTs](06-types-and-adt.md) — Algebraic data types, `value`, `data`, and `impl`
7. [A Real Program](07-real-program.md) — A complete working example from stdin to output
8. [Testing your code](08-testing.md) — `tyra test`, assertions, TAP output
9. [Project Lifecycle](09-project-lifecycle.md) — `tyra new`, `tyra mod`, dependencies, builds

## Quick Reference

```tyra
# Hello, World
print("Hello, World!\n")

# Functions
fn greet(_ name: String) -> String
  "Hello, #{name}!"
end

# Variables
let x = 42
mut count = 0
count = count + 1

# Pattern matching
match list.get(items, 0)
when Some(v)
  print("got: #{v}")
when None
  print("empty")
end
```

## What Makes Tyra Different?

- **No `null`** — `Option<T>` makes absence explicit
- **No exceptions** — `Result<T, E>` makes errors explicit
- **`end` blocks, not braces** — unambiguous block boundaries
- **Explicit argument labels** — call sites are self-documenting
- **Value vs. data distinction** — memory semantics are visible in the type
- **Single toolchain** — `tyra check`, `tyra run`, `tyra build`, `tyra fmt`, `tyra test`, `tyra new`, `tyra mod`
