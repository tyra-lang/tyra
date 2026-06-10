# Hello, World

This page covers the essentials: running your first program, variables, types, string interpolation, and functions.

## Your First Program

The simplest Tyra program is a single statement at the top level:

```tyra
print("Hello, World!\n")
```

Save it as `hello.ty` and run:

```bash
tyra run hello.ty
```

There is no required `main` function for small scripts — top-level statements execute in order. For larger programs, you define a `main` function:

```tyra
fn main() -> Unit
  print("Hello, World!\n")
end
```

> **NOTE:** `\n` inside a string literal is a newline character. `print` does not add a newline automatically.

## `tyra run` vs `tyra build`

| Command | What it does |
|---|---|
| `tyra run file.ty` | Compile and immediately execute |
| `tyra build file.ty` | Compile to a native binary (`./out` by default) |
| `tyra build -o myapp file.ty` | Compile to a named binary |

Use `tyra run` during development. Use `tyra build` when you want a binary to distribute or deploy.

## Variables

Use `let` to declare an immutable binding. The type is inferred from the value:

```tyra
let x = 42
let greeting = "Hello"
let flag = true
```

Use `mut` to declare a mutable binding that can be reassigned:

```tyra
mut count = 0
count = count + 1
count = count + 1
print("count: #{count}\n")
```

> **NOTE:** `let` bindings cannot be reassigned. Attempting to assign to a `let` binding is a compile error.

## Basic Types

| Type | Example | Notes |
|---|---|---|
| `Int` | `42`, `-7` | 64-bit signed integer |
| `Float` | `3.14`, `-0.5` | 64-bit IEEE 754 double |
| `Bool` | `true`, `false` | |
| `String` | `"hello"` | UTF-8 |
| `Unit` | `()` | The "no value" type |

Type annotations are optional when the type can be inferred, but you can be explicit:

```tyra
let x: Int = 42
let name: String = "alice"
```

> **TIP:** `Float` does not support `==` comparison. Use `import float` and call `float.eq(a, b)` for exact equality, or `float.approx_eq(a, b, eps)` for tolerance-based comparison.

## String Interpolation

Embed any expression inside a string using `#{...}`:

```tyra
let name = "World"
let n = 42
print("Hello, #{name}!\n")
print("The answer is #{n}.\n")
print("Double: #{n * 2}\n")
```

Output:

```
Hello, World!
The answer is 42.
Double: 84
```

The expression inside `#{...}` can be any value that has a string representation — integers, booleans, strings, and types implementing `Stringable`.

## Functions

Declare a function with `fn`, parameter list, return type, body, and `end`:

```tyra
fn add(_ a: Int, _ b: Int) -> Int
  a + b
end
```

The last expression in the body is the return value — no `return` keyword needed. You can use `return` for early exit:

```tyra
fn safe_divide(_ a: Int, _ b: Int) -> Int
  if b == 0
    return 0
  end
  a / b
end
```

### Argument Labels

Function parameters have an **argument label** (the name used at the call site) and a **parameter name** (the name used inside the body). The `_` label means the argument is passed without a label:

```tyra
fn greet(_ name: String) -> String
  "Hello, #{name}!"
end

# Call: no label needed
let msg = greet("Alice")
print("#{msg}\n")
```

With explicit labels:

```tyra
fn repeat(text s: String, times n: Int) -> String
  # body uses s and n
  s
end

# Call: use the labels
let r = repeat(text: "hi", times: 3)
```

> **TIP:** Using `_` as the label is the most common style for straightforward functions. Explicit labels improve readability at call sites when the meaning of an argument is not obvious.

### Functions that Return Unit

Functions that produce no meaningful value return `Unit`:

```tyra
fn log(_ message: String) -> Unit
  print("[LOG] #{message}\n")
end

log("starting up")
```

## Putting It Together

```tyra
fn celsius_to_fahrenheit(_ c: Int) -> Int
  c * 9 / 5 + 32
end

fn main() -> Unit
  let boiling = celsius_to_fahrenheit(100)
  let freezing = celsius_to_fahrenheit(0)
  print("Boiling: #{boiling} F\n")
  print("Freezing: #{freezing} F\n")
end
```

Output:

```
Boiling: 212 F
Freezing: 32 F
```

## Next Steps

Continue to [Control Flow](03-control-flow.md) to learn `if`, `for`, `while`, and `match`.
