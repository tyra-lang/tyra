# Tyra Language Specification

- **Version**: 0.4
- **Status**: Stable
- **Last updated**: 2026-05-26

> This is the English translation of the Tyra Language Specification.
> The Japanese version (`docs/spec/ja/language-spec.md`) is the **authoritative source**.
> If this translation conflicts with the Japanese version, the Japanese version prevails.

## 1. Goals

Tyra aims to simultaneously satisfy the following properties:

- Readable syntax inspired by Ruby
- Practical static typing in the spirit of TypeScript
- Simple build / test / fmt / deploy operations like Go
- Less strict ownership rules than Rust
- Less ambiguity than Python
- Consistent interpretation in collaboration between humans and AI

In one phrase, Tyra is a **readable, type-safe, easy-to-distribute practical language with minimal ambiguity**.

This specification defines the semantics of the language. The primary implementation target is native compilation, and the reference implementation uses LLVM.

---

## 2. Design Principles

### 2.1 Explicitness

- Syntax must be interpretable in only one way
- No call-site omissions, implicit conversions, or runtime metaprogramming
- `null` does not exist in the language
- No truthy/falsy semantics

### 2.2 Readability

- Blocks are closed with `end`
- Avoid excessive symbolic syntax
- Express the meaning of APIs through argument labels and types

### 2.3 Practical types

- Static typing
- Strong local type inference
- `Option` and `Result` as standard concepts
- Argument types and return types are required for public APIs

### 2.4 Operational simplicity

- A single official toolchain
- Standardized formatter that constrains code style choices
- The reference implementation prioritizes producing a single native binary

### 2.5 AI-friendly

- Same input produces the same AST as much as possible
- Unified naming conventions, imports, type expressions, and error handling patterns
- Prioritize completion and readability over DSL-level expressive freedom

---

## 3. Non-goals

Tyra v0.1 does not aim for the following:

- Extremely low-level control for OS or kernel development
- Rust-style ownership / borrow checker
- Python-style REPL-centric design
- A frontend-only language
- A sophisticated macro system
- Inheritance-based OOP
- Runtime reflection

---

## 4. Intended use cases

The primary targets for Tyra are:

- Web backends / API servers
- CLI tools
- Internal business applications
- Small to medium-scale services

---

## 5. Lexical rules

### 5.1 Identifiers

- Type names: `PascalCase`
- Function and variable names: `snake_case`
- Constant names: `UPPER_SNAKE_CASE`
- Module names: `snake_case`

### 5.2 Reserved words

`fn`, `data`, `value`, `type`, `trait`, `impl`, `let`, `mut`, `if`, `else`, `match`, `when`, `for`, `in`, `while`, `return`, `defer`, `async`, `await`, `spawn`, `import`, `export`, `and`, `or`, `not`, `true`, `false`, `end`

### 5.3 Comments

```tyra
# line comment
```

Multi-line comments are not adopted in v0.1.

### 5.4 Statement termination

- Statements are separated by newlines
- `,` is used only when necessary
- `;` is not adopted
- Newlines are not statement separators inside `(` `)`, `[` `]`, or `{` `}`
- Trailing commas are allowed

---

## 6. Block syntax

Tyra uses keyword-and-`end` blocks.

```tyra
if ready
  run()
else
  wait()
end
```

Reasons:

- The structure is easy for humans to follow
- Indentation alone does not carry meaning
- Block boundaries are clear for AI

### 6.1 Top-level executable statements

In an entry-point file, statements other than declarations may be written at the top level. These are treated as the body of an implicit `fn main() -> Unit` (see ADR-0006 for the rationale).

An **expression statement** is an expression placed in statement position. Its value is discarded. The executable statements permitted at the top level are: expression statements, `let`/`mut` bindings, `if`, `match`, `for`, `while`, and `defer`.

```tyra
# Entry-point file: no fn main needed
print("hello, tyra")
```

The above is equivalent to:

```tyra
fn main() -> Unit
  print("hello, tyra")
end
```

Declarations (`fn`, `type`, `value`, `data`, `trait`, `impl`, `import`) are not executable statements. `export` is a modifier on declarations, not an executable statement. Declarations remain outside the implicit main; only executable statements become its body. Mixing declarations and executable statements is permitted. However, `fn main` is an exception and cannot coexist with top-level executable statements (see the rules below).

Forward references apply only to top-level declaration names (function names, type names, trait names, etc.). `let`/`mut` bindings in top-level executable statements are local variables of the implicit main and are not eligible for forward reference.

```tyra
# Mixing declarations and executable statements:
# fib is defined after the print but can still be referenced
print("fib(10) = #{fib(10)}")

fn fib(_ n: Int) -> Int
  match n
  when 0
    0
  when 1
    1
  when _
    fib(n - 1) + fib(n - 2)
  end
end
```

Rules:

- `fn main` may only be defined in the entry-point file. A `fn main` in an imported module file is a compile error
- `fn main` may not have `export`. `main` is reserved for the entry point and is not intended for external visibility
- `fn main` and top-level executable statements may not coexist in the same file (compile error)
- Top-level executable statements are type-checked as the body of an implicit `fn main() -> Unit`; therefore `?`, `.await`, and `return` are not usable
- Imported module files may not contain top-level executable statements (§13.1)
- Top-level `let`/`mut` bindings in an entry-point file are local variables of the implicit main, not module-scope bindings. Evaluated bindings exist only inside the implicit main of the entry-point file
- Module files may not contain any top-level executable statements, including `let`/`mut` bindings
- `defer` is usable at the top level. It executes in LIFO order on exit from the implicit main's scope (which typically corresponds to just before program termination)
- The entry-point file is designated by the toolchain. Application packages require exactly one entry point. Library packages do not require an entry point

---

## 7. Values and variables

### 7.1 Variable bindings

```tyra
let name = "tyra"
mut count = 0
```

- `let` forbids reassignment of the binding
- `mut` permits reassignment of the binding
- The default is immutable binding

### 7.2 Primitive types

- `Int`
- `Float`
- `Bool`
- `String`
- `Rune`
- `Bytes`
- `Unit`
- `Never`

`Int` is a 64-bit signed integer; `Float` is IEEE 754 double precision.
Integer literals default to `Int` and floating-point literals default to `Float` when no contextual type is available.
`Rune` is a 32-bit value representing a Unicode scalar value. Grapheme clusters are the responsibility of `String`.

`Float` does not have the `Eq` ability. This avoids the contradiction between IEEE 754's `NaN != NaN` and structural equality. For Float comparison, use functions in the standard library `float` module (see ADR-0002 for the rationale).

`Unit` is written with the literal `()`.

```tyra
let result: Result<Unit, Error> = Ok(())
```

`Never` is a type with no values, indicating that a function does not return. `Never` is a subtype of every type.

```tyra
fn panic(_ message: String) -> Never
  ...
end

let x: Int = if condition
  42
else
  panic("unreachable")  # Never coerces to Int
end
```

### 7.3 Strings

Regular strings support interpolation.

```tyra
let msg = "hello, #{user.name}"
```

Escape sequences:

- `\n` — newline
- `\t` — tab
- `\r` — carriage return
- `\\` — backslash
- `\"` — double quote
- `\0` — null byte
- `\u{XXXX}` — Unicode code point (1-6 hex digits)

#### Raw strings

Raw strings are written as `r"..."`. Escape sequences and string interpolation are not processed.

```tyra
let pattern = r"\d{3}-\d{4}"
let path = r"C:\Users\mika\docs"
let query = r"SELECT * FROM users WHERE name = '#{not_interpolated}'"
```

Rules:

- Inside `r"..."`, backslashes and `#{}` are treated as literal characters
- The `"` character cannot be included (no escape mechanism is provided)
- The type of a raw string is `String` (the same as a regular string)

Multi-line strings are not adopted in v0.1.

---

## 8. Type system

### 8.1 Basic policy

- Static typing
- Strong local type inference
- Argument types and return types are required for public functions
- Local variable types may be inferred

```tyra
let x = 10
let y: Int = 20
```

### 8.2 No nullable types

`null` does not exist. Absence is expressed via `Option<T>`.

```tyra
let user: Option<User> = repo.find_user(id)
```

The `T?` syntax is not adopted in v0.1. To keep types explicit, `Option<T>` is the only form.

### 8.3 Result

Recoverable failures are expressed via `Result<T, E>`.

```tyra
fn parse_int(text: String) -> Result<Int, ParseError>
  ...
end
```

### 8.4 Generics

Type application uses angle brackets at declaration sites and type-annotation positions.

```tyra
fn first<T>(items: List<T>) -> Option<T>
  ...
end
```

Tyra has two kinds of constraint: **trait** and **ability**.

- trait: represents a replaceable behavior
- ability: a compiler-known constraint that represents a structural property of a type, and **cannot be implemented with `impl`**

The abilities in v0.1 are `Eq`, `Hash`, `Ord`, and `Debug`.

```tyra
fn contains<T: Eq>(_ items: List<T>, _ target: T) -> Bool
  ...
end
```

When multiple constraints are required, combine them with `+`.

```tyra
fn deduplicate<T: Eq + Hash>(_ items: List<T>) -> List<T>
  ...
end
```

Rules:

- Each type parameter may have 0, 1, or 2 constraints
- Constraint syntax is `<T: Constraint>` or `<T: A + B>`
- `Constraint` may be either a trait or an ability
- 3 or more constraints, `where` clauses, associated types, and higher-kinded types are not adopted
- Type application: `List<Int>`
- Indexing: `items[0]`
- List literals: `[1, 2, 3]`
- Explicit type application in expression position uses turbofish: `parse::<Int>(text)`
- Forms like `foo<A, B>(x)` (angle brackets in expression position without turbofish) are not allowed, to avoid ambiguity

### 8.5 Type aliases and Union / ADT

`type` is used for both type aliases and ADTs.

Type aliases:

```tyra
type UserId = Int
type Handler = fn(Request) -> Response
```

Union / ADT:

```tyra
type Payment =
  | Card(last4: String)
  | Bank(bank_name: String)
  | Cash
```

- ADTs use data semantics (reference type, GC-managed) (see ADR-0001 for the rationale)
- Recursive self-references are allowed
- `match` must be exhaustive
- Tagged unions are the standard
- Constructor patterns with named fields use named destructuring by default
- `when Card(last4)` is shorthand for `when Card(last4: last4)`
- An ADT composed only of variants whose fields all satisfy `Eq` automatically gains the `Eq` ability
- The `Ord` ability is not automatically derived (same rule as `data`)

#### Constructor calls

ADT variant constructors are called with the qualified form `TypeName.VariantName`.

```tyra
let c = Color.Red
let p = Payment.Card(last4: "1234")
let e = AppError.NotFound
```

Exception: the variants of `Option` and `Result` (`Some`, `None`, `Ok`, `Err`) are included in the prelude and may be used unqualified.

```tyra
let user: Option<User> = Some(find_user())
let result: Result<Int, Error> = Ok(42)
let empty: Option<Int> = None
```

In `match` patterns, variants are written unqualified because the variant can be uniquely identified from the match target's type.

```tyra
let p = Payment.Card(last4: "1234")   # construction: qualified

match p
when Card(last4: last4)               # pattern: unqualified
  "card: #{last4}"
when Cash
  "cash"
end
```

### 8.6 value and data

Tyra distinguishes between `value` and `data`.

#### value

- Value type
- Copied semantically on assignment, parameter passing, and return
- Implementations may apply copy-elision optimization
- Fields are always immutable
- Cannot have direct recursive self-reference
- For recursive structures, use `data`
- Automatically gains the `Eq` ability if all fields satisfy `Eq`
- Automatically gains the `Hash` ability if all fields satisfy `Hash`
- Automatically gains the `Ord` ability **only for single-field `value` types** whose field satisfies `Ord`
- Automatically gains the `Debug` ability if all fields satisfy `Debug`
- A type with the `Hash` ability necessarily has the `Eq` ability
- `==` is available when the type has the `Eq` ability
- `===` does not exist
- A built-in `copy(...)` is automatically provided

```tyra
value Point
  x: Float
  y: Float
end
```

```tyra
value Money
  cents: Int
end

let p1 = Point(x: 1.0, y: 2.0)
let p2 = p1.copy(x: 3.0)
```

#### copy for value

A built-in `copy(...)` is automatically provided for `value` types.

- `copy` accepts every field as an optional named argument
- Omitted fields inherit the value from the receiver
- All arguments must be named arguments (positional arguments are not allowed)
- Argument names must match the field names of the target `value` type
- The same field cannot be specified more than once
- `copy()` (with no arguments) returns a new instance equivalent to the receiver
- The return type is the same `value` type as the receiver

```tyra
value Point
  x: Float
  y: Float
end

let p1 = Point(x: 1.0, y: 2.0)
let p2 = p1.copy(x: 3.0)         # Point(x: 3.0, y: 2.0)
let p3 = p1.copy(x: 0.0, y: 0.0) # Point(x: 0.0, y: 0.0)
let p4 = p1.copy()               # Point(x: 1.0, y: 2.0)
```

`data` types do not receive a built-in `copy`. Updates to `data` are performed by direct assignment to `mut` fields.

#### data

- Reference type
- GC-managed
- Allows recursive structures and sharing
- Fields are immutable by default; mutable fields are marked with `mut`
- `===` compares reference identity
- Automatically gains the `Eq` ability if all fields satisfy `Eq`
- Automatically gains the `Hash` ability **only when all fields are immutable AND all fields satisfy `Hash`**
- A `data` type with any `mut` field cannot have the `Hash` ability
- The `Ord` ability is not automatically derived
- Automatically gains the `Debug` ability if all fields satisfy `Debug`
- A type with the `Hash` ability necessarily has the `Eq` ability
- `==` is available when the type has the `Eq` ability
- When ordering is needed, use key-function APIs such as `sort_by`, `min_by`, and `max_by`

```tyra
data User
  id: Int
  mut name: String
end
# User has a mut field, so it cannot have the Hash ability
# Set<User> and Map<User, V> are compile errors

data Config
  host: String
  port: Int
end
# Config has all immutable fields satisfying Hash, so it gains Hash automatically
# Set<Config> is usable
```

#### Field update rules

- Fields of a `value` cannot be updated
- A `data` field can be updated only if the field itself is `mut` **and** the binding receiving it is `mut`
- Function parameter bindings are immutable by default
- This rule is stricter than reference types in Java, Kotlin, and Swift; Tyra makes mutable state explicit even at the binding level

```tyra
mut user = User(id: 1, name: "mika")
user.name = "mika sato"
```

### 8.7 Trait

`trait` is used in place of inheritance.

```tyra
trait Stringable
  fn to_string(self) -> String
end
```

Implementation:

```tyra
impl Stringable for User
  fn to_string(self) -> String
    "#{self.name}"
  end
end
```

Rules:

- `trait` can be implemented for both `value` and `data`
- Trait dispatch in v0.1 is static dispatch only
- Trait objects do not exist in v0.1
- `self` is passed by value for `value` and by reference for `data`
- Function name overloading is forbidden, but trait dispatch is not considered overloading
- When a single collection must hold heterogeneous elements, use an ADT instead of trait objects

### 8.8 Nominal typing

v0.1 adopts nominal typing. The reasons are:

- Keep error messages simple
- Reduce type-inference variance for AI
- Make compiler implementation clearer

---

## 9. Functions

### 9.1 Definition

```tyra
fn add(_ x: Int, _ y: Int) -> Int
  x + y
end
```

`fn main` is the entry point of the program. `main` may only be defined in the entry-point file and may not have `export` (§6.1). When the entry-point file contains top-level executable statements, `fn main` may be omitted. In that case the statements are normalized to an implicit `fn main() -> Unit`. When `Result` return or `async` is needed, define `fn main` explicitly.

```tyra
# Explicit main: can return Result
fn main() -> Result<Unit, AppError>
  let config = read_config("app.conf")?
  start_server(config)?
end

# Explicit async main
async fn main() -> Result<Unit, AppError>
  let app = server.new()
  app.listen(port: 8080).await?
end
```

### 9.2 Calls

```tyra
let n = add(1, 2)
```

- Parentheses are required
- Even zero-argument calls require `name()`
- Ruby-style omissions like `foo bar` are not allowed

### 9.3 Argument labels

Tyra adopts rules close to Swift's.

- Public function parameters require labels by default
- A parameter prefixed with `_` is a positional parameter
- When the external label and internal name are the same, write it once
- When the external label and internal name differ, write both
- Labeled parameters may follow positional parameters
- Positional parameters may not follow labeled parameters
- No label is omittable in any given call

```tyra
fn create_user(name: String, admin: Bool) -> User
  ...
end

fn set_position(to target: Point) -> Unit
  ...
end

fn add(_ x: Int, _ y: Int) -> Int
  x + y
end

create_user(name: "mika", admin: true)
set_position(to: point)
add(1, 2)
```

Function parameters are always immutable bindings. When mutability is needed, rebind with `mut` inside the function body.

```tyra
fn process(_ x: Int) -> Int
  mut count = x
  count = count + 1
  count
end
```

### 9.4 Function types and anonymous functions

`fn` uniformly represents function definitions, function types, and anonymous functions.

Function types are written as `fn(...) -> T`.

```tyra
fn map<T, U>(_ items: List<T>, _ f: fn(T) -> U) -> List<U>
  ...
end
```

Anonymous functions use `fn` expressions.

```tyra
let double = fn(_ x: Int) -> Int
  x * 2
end
```

Closure capture rules:

- Captures are read-only by default
- Reassigning a captured `mut` binding from inside a closure is not allowed
- Captures of `value` types are semantically copied
- Captures of `data` types are treated as references

### 9.5 return

The last expression is implicitly returned.

```tyra
fn abs(_ x: Int) -> Int
  if x >= 0
    x
  else
    -x
  end
end
```

`return` may be used when needed.

---

## 10. Control flow

### 10.1 Expressions and statements

Tyra is expression-oriented.

- `if` and `match` are expressions
- In statement position, `if` is treated as a statement with value `Unit` (see §10.2 for details)
- `while` and `for` are statements with value `Unit`
- The value of a block is the value of its last expression

#### Arithmetic operators

Five infix arithmetic operators: `+`, `-`, `*`, `/`, `%`. Both
operands must have the same numeric type: `Int` × `Int` or
`Float` × `Float` (`%` is `Int` × `Int` only). Mixed-type
arithmetic must go through an explicit `Into<F>` conversion
(§12.2).

```tyra
let a = 10 + 3       # 13
let b = 10 - 3       # 7
let c = 10 * 3       # 30
let d = 10 / 3       # 3  (Int division truncates toward zero)
let e = 10 % 3       # 1  (Int remainder; sign follows the dividend)
```

- `/` truncates toward zero for `Int` × `Int`, and uses IEEE 754
  division for `Float` × `Float`.
- `%` is `Int` × `Int` only. The sign of the result follows the
  dividend (LLVM `srem`, same as C99). Float remainder is not
  provided in v0.1.
- Division or remainder by zero is implementation-defined (LLVM
  `sdiv` / `srem` semantics, typically an abnormal process
  termination). Checking against zero is the caller's
  responsibility.

#### Logical operators

Logical operators use the keywords `and`, `or`, and `not`.

```tyra
if age >= 18 and country == "JP"
  grant_access()
end

if not user.is_banned
  allow_login()
end

if score < 60 or has_warnings
  request_review()
end
```

- `and` — logical AND (short-circuit evaluation)
- `or` — logical OR (short-circuit evaluation)
- `not` — logical NOT (prefix)
- Both operands must be `Bool`
- Precedence: `not` > `and` > `or`

`or` is used only as the logical OR operator.

### 10.2 if

```tyra
if ok
  handle_ok()
else
  handle_error()
end
```

- The condition must be of type `Bool`
- No truthy/falsy semantics
- `if` is treated as an expression in expression position and as a statement in statement position
- In expression position, `else` is required and the value types of both branches must match
- In statement position (used for side effects only), `else` may be omitted; the value is `Unit`

Examples:

```tyra
# Expression position: else required
let label = if x > 0
  "positive"
else
  "non-positive"
end

# Statement position: else may be omitted
if user.is_admin
  log.info("admin login")
end
```

A position is considered an "expression position" when the value of the `if` is bound, used as a function argument, used as a return value, or otherwise used as an expression. Otherwise it is a "statement position".

#### else if

`if` may be placed immediately after `else`. In this case only one `end` is written for the entire chain.

```tyra
if x > 0
  "positive"
else if x < 0
  "negative"
else
  "zero"
end
```

This differs from nesting an `if` inside an `else` block, which would require two `end` keywords.

### 10.3 match

```tyra
match result
when Ok(value)
  render(value)
when Err(err)
  log_error(err)
end
```

- Must be exhaustive
- Usable on ADTs, enums, and literals
- Use the wildcard `_` for types where exhaustiveness is impossible
- Patterns may be nested
- Like `if`, `match` is treated differently in expression position and statement position
- In expression position, the value types of all `when` arms must match
- In statement position, the value of each `when` arm is discarded and the overall value is `Unit`
- v0.1 has no guard clauses; use `if / else` for conditional branching

### 10.4 while

```tyra
while running
  tick()
end
```

The value is `Unit`.

### 10.5 for

```tyra
for item in items
  print(item)
end
```

- C-style `for (;;)` is not adopted
- The value is `Unit`
- `continue` transfers control to the next iteration of the enclosing loop (valid inside while/for only; E0215 outside a loop)

---

## 11. Collections

> **Design intent vs. v0.1 implementation scope**: This section describes the full language design target. For the v0.1 frozen scope, see the callout at the end of this section.

Standard collections (design target):

- `List<T>`
- `Map<K, V>` — fully generalized in v0.6.0 for arbitrary `K: Eq + Hash` / arbitrary `V` (§17.3.6)
- `Set<T>` — added in v0.6.0 for arbitrary `T: Eq + Hash` (§17.3.7)

Literals:

```tyra
let nums = [1, 2, 3]
let scores: Map<String, Int> = {"alice": 92, "bob": 85}
let by_id: Map<Int, String> = {1: "mika", 2: "jun"}   # v0.6.0+
```

- Map literal keys may be arbitrary expressions (`K: Eq + Hash` required)
- The key type `K` of `Map<K, V>` must satisfy `Hash`
- Indexing uses `items[index]`
- `items[index]` panics on out-of-bounds access
- For safe access, use `items.get(index)`, which returns `Option<T>`

```tyra
let x = items[0]           # panics if out of bounds
let y = items.get(0)       # returns Option<T>
let z = items.get(0)?      # Option early return (when the enclosing function returns Option)
```

> **Implementation scope notes**:
> - `List<T>`: generic `[]` / `.get(index)` / `for` are available. The `list` module functions (`list.push` / `sum` / `max` / `min` / `contains` / `index_of`) are frozen for **`List<Int>` only** (§17.3.5). `List<String>` and other element types can be iterated with `for` but the `list.*` functions do not apply.
> - `Map<K, V>`: fully generalized in v0.6.0 for arbitrary K / V (§17.3.6). `remove` / iteration are a later release.
> - `Set<T>`: added in v0.6.0 (§17.3.7). Set-literal syntax and set operations are a later release.

---

## 12. Error handling

### 12.1 Principles

- Predictable failures use `Result`
- Unpredictable bugs cause panic
- Exception mechanisms are not adopted in v0.1
- `Option` represents absence; `Result` represents an error. `?` works on both

#### panic

`panic` is a function that terminates the program abnormally. It is not a macro.

```tyra
fn panic(_ message: String) -> Never
```

```tyra
fn divide(_ a: Int, _ b: Int) -> Int
  if b == 0
    panic("division by zero")
  end
  a / b
end
```

- `panic` is included in the `core` module and is always available via the prelude
- Its return type is `Never`, so it can be used in any position where any type is expected
- `panic` indicates an unrecoverable state. Use `Result` for recoverable failures.

### 12.2 Propagation operator

`?` works on both `Result` and `Option`.

#### ? on Result

```tyra
fn load_user(_ id: Int) -> Result<User, AppError>
  let row = db.find(id)?
  decode_user(row)?
end
```

Rules:

- `expr?` is usable when `expr` has type `Result<T, E>`
- The enclosing function's return type must be `Result<U, F>`
- `E` must implement `Into<F>`
- Evaluates to `value` when the result is `Ok(value)`
- Returns `Err(e.into())` early when the result is `Err(e)`

#### ? on Option

```tyra
fn user_name(_ id: Int) -> Option<String>
  let user = repo.find(id)?
  Some(user.name)
end
```

Rules:

- `expr?` is also usable when `expr` has type `Option<T>`
- The enclosing function's return type must be `Option<U>`
- Evaluates to `value` when the option is `Some(value)`
- Returns `None` early when the option is `None`

#### Into

`Into<T>` is a standard trait included in the `core` prelude.

```tyra
trait Into<T>
  fn into(self) -> T
end
```

Rules:

- `Into<T> for T` is automatically provided by the compiler
- In v0.1 the `?` operator may treat `Into` specially
- `From` is not adopted in v0.1

### 12.3 defer

```tyra
fn handle() -> Result<Unit, AppError>
  defer print("handler exited")
  let text = fs.read_to_string("app.conf")?
  ...
end
```

Rules:

- `defer` runs in LIFO order when the current scope exits
- The GC handles only memory reclamation
- Resource release is via `defer` or explicit close
- v0.1 does not have finalizers

---

## 13. Modules

### 13.1 Files and modules

- One file = one module
- The file name matches the module name
- Module files (those that are `import`-ed) may only contain declarations (`fn`, `type`, `value`, `data`, `trait`, `impl`)
- Module files may not contain top-level executable statements or `let`/`mut` bindings
- v0.1 does not define module-level initialization semantics

### 13.2 import

```tyra
import http.server
import app.user_repo as user_repo
```

Rules:

- `import a.b.c` introduces the trailing name `c` into the current scope
- Aliases via `as` are allowed
- Fully-qualified references such as `a.b.c.name` are also allowed
- Wildcard imports are forbidden
- Relative imports are not adopted in v0.1

### 13.3 export

```tyra
export fn serve(port: Int) -> Result<Unit, ServerError>
  ...
end
```

The default is private.

Visibility in v0.1 has only two levels: `export` and private. There is no intermediate `internal`-style visibility.

---

## 14. Concurrency

### 14.1 Approach

- `async` / `await` are standard features
- Message passing is preferred over shared mutable state
- The actor model is provided as a library, not a standard abstraction in v0.1
- The reference implementation uses an M:N work-stealing scheduler

### 14.2 async function

```tyra
async fn fetch_user(_ id: Int) -> Result<User, HttpError>
  ...
end
```

Type rules:

- The result type of calling `async fn f(...) -> T` is `Task<T>`
- async functions may freely call sync functions
- sync functions may produce `Task<T>`, but `.await` is usable only inside async functions
- `main` may be either `fn main() -> Result<Unit, E>` or `async fn main() -> Result<Unit, E>`

Examples:

- The result type of calling `async fn fetch_user(_ id: Int) -> Result<User, HttpError>` is `Task<Result<User, HttpError>>`
- `fetch_user(id).await?` is evaluated in the following order:

  1. `fetch_user(id)` -> `Task<Result<User, HttpError>>`
  2. `.await` -> `Result<User, HttpError>`
  3. `?` -> `User`

### 14.3 await

`await` is postfix.

```tyra
let user = fetch_user(id).await?
```

Rules:

- `.await` is a postfix operator
- `.await` binds tighter than `?`
- `fetch_user(id).await?` parses as `(fetch_user(id).await)?`

### 14.4 spawn

v0.1 provides `spawn`.

```tyra
let task = spawn fetch_user(id)
let result = task.await?
```

Rules:

- `spawn` only accepts a function call (arbitrary expressions are not allowed)
- `spawn f(args)` runs the function `f` concurrently and returns `Task<T>`
- If `f` is a sync function, its execution is performed on a separate task and the result is wrapped in `Task<T>`
- If `f` is an async function, the runtime performs the equivalent of `.await` internally and wraps the final result in `Task<T>`
- Task cancellation is not a language feature in v0.1
- Cancellation is left to a future library API

---

## 15. Memory management

### 15.1 Basic policy

The reference implementation uses tracing GC.

- Generational
- Low-latency oriented
- Minimizes runtime pauses

### 15.2 No ownership

- No borrow checker
- Safety is improved through explicit `mut` and the value/data distinction

### 15.3 Value type optimization

- `value` types may be stack-allocated
- Escape analysis reduces unnecessary heap allocation
- The internal representation of `List<value T>` is implementation-defined
- Layout optimization must not affect semantics

---

## 16. AI-friendly rules

Tyra includes "easy for AI to handle" as a design requirement, not just for humans.

### 16.1 Adopted rules

- Calls always require parentheses
- Block terminator is `end`
- No truthy/falsy
- No `null`
- Public APIs require type annotations
- Fixed import form
- Layout is fixed by the formatter
- No function name overloading

### 16.2 Forbidden

- Runtime `eval`
- Dynamic method definition
- Heavy use of implicit receivers
- Multiple equivalent syntactic forms

---

## 17. Standard library

The standard library is split into two tiers (see ADR-0003 for the rationale).

### 17.1 Tier 1: included in the language specification

These are required by the compiler or type checker and are defined as part of the language specification.

#### core

```tyra
# I/O
export fn print<T: Debug>(_ value: T) -> Unit
export fn println<T: Debug>(_ value: T) -> Unit
export fn eprint<T: Debug>(_ value: T) -> Unit
export fn eprintln<T: Debug>(_ value: T) -> Unit

# Program control
export fn panic(_ message: String) -> Never
```

`()` is the literal of the `Unit` type.

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

#### Option and Result

```tyra
type Option<T> =
  | Some(value: T)
  | None

type Result<T, E> =
  | Ok(value: T)
  | Err(error: E)
```

#### prelude

The following are auto-imported into every module. No `import` is needed.

Standard traits:

- `Into<T>`
- `Stringable`

Compiler-known standard abilities:

- `Eq`
- `Hash`
- `Ord`
- `Debug`

ADT variants:

- `Some`, `None` (variants of `Option`)
- `Ok`, `Err` (variants of `Result`)

Functions:

- `print`, `println`, `eprint`, `eprintln`
- `panic`

Operator correspondences:

- `==`, `!=` -> `Eq`
- `<`, `<=`, `>`, `>=` -> `Ord`
- `+`, `-`, `*`, `/` -> built-in numeric operations only; no operator overloading
- `and`, `or`, `not` -> built-in logical operations on `Bool` only

### 17.2 Tier 2: defined in separate documents

These modules are practically important but do not affect language semantics. Their APIs are defined separately in `docs/stdlib/`.

- `string` — string operations (len, trim, contains, starts_with, etc.; v0.1 API frozen in §17.3.4)
- `list` — `List<Int>` operations (push, sum, max, min, contains, index_of; v0.1 API frozen in §17.3.5, generic `List<T>` deferred to §22)
- `Map<K, V>` — arbitrary `K: Eq + Hash`, arbitrary `V`; fully generalized in v0.6.0 (§17.3.6)
- `Set<T>` — arbitrary `T: Eq + Hash`; added in v0.6.0 (§17.3.7)
- `collections` — methods on `List`, `Map`, `Set` (sort_by, min_by, max_by, map, filter, etc.)
- `float` — Float comparison functions (eq, approx_eq, is_nan, etc.; see ADR-0002)
- `json` — JSON parsing (v0.1 API frozen in §17.3)
- `http` — HTTP server and client (v0.1 API frozen in §17.3)
- `fs` — file system operations (v0.1 API frozen in §17.3)
- `time` — time and duration (added in v0.6.0: §17.3.8)
- `log` — logging (added in v0.6.0: §17.3.9)
- `test` — testing framework

Principles:

- Things commonly used in production are in the standard library
- Reproducibility is preferred over freedom of dependency choice

### 17.3 Tier 2 APIs frozen in v0.1

M10 freezes minimal APIs for `fs` and `json`, M11 freezes
`http.client` / `http.server`, and a minimal `string` API is also
frozen, as part of the language specification. The remaining Tier 2
modules (`collections`, `time`, `test`, `log`, `float`) will be
finalized in later milestones.

#### 17.3.1 fs

Callers `import fs` and use the module-qualified form
`fs.read_to_string(...)`. The declarations below are excerpted from
`stdlib/fs.tyra`.

```tyra
# stdlib/fs.tyra
export fn read_to_string(_ path: String) -> Result<String, FsError>
export fn write_string(_ path: String, _ contents: String) -> Result<Unit, FsError>
export fn exists(_ path: String) -> Bool

export type FsError =
  | NotFound(path: String)
  | PermissionDenied(path: String)
  | IoError(message: String)
```

- `read_to_string` / `write_string` read or write a file in full.
  Large-file or streaming workloads are out of scope for v0.1 (M11+).
- `exists` does not distinguish files from directories.
- `FsError.IoError` is a catch-all for every failure that is not
  `NotFound` or `PermissionDenied`; v0.1 does not expose a detailed
  errno enumeration.

#### 17.3.2 json

Callers `import json` and use the module-qualified form
`json.parse(...)` / `json.Value`. The declarations below are excerpted
from `stdlib/json.tyra`.

```tyra
# stdlib/json.tyra
export data Value
  _handle: Int
end

export type JsonError =
  | ParseFailed(message: String, line: Int, col: Int)
  | TypeMismatch(expected: String, got: String)
  | MissingKey(key: String)

export fn parse(_ text: String) -> Result<Value, JsonError>

impl ValueOps for Value
  fn kind(self) -> String                # "null" | "bool" | "int" | "string" | "array" | "object"
  fn as_string(self) -> Option<String>
  fn as_int(self) -> Option<Int>
  fn as_bool(self) -> Option<Bool>
  fn get(self, key: String) -> Option<Value>      # object only
  fn at(self, _ index: Int) -> Option<Value>      # array only
end
```

- Numbers are parsed as `Int` only. JSON floating-point literals surface
  as `ParseFailed` (a `Float` accessor is deferred to v0.2+).
- String `\uXXXX` escapes support BMP and surrogate pairs (RFC 8259 §7).
- `TypeMismatch` / `MissingKey` are never returned by the stdlib itself
  (`as_*` / `get` / `at` return `None`). They are ADT variants provided
  for callers to use as their own error types.
- `json.Value` behaves as a GC-managed opaque handle (§8.5). In v0.1 a
  parsed tree lives for the duration of the process (explicit
  deallocation is not supported). See `runtime/src/stdlib_json.rs` for
  implementation notes.

#### 17.3.3 http

Callers `import http.client` / `import http.server` and use the
module-qualified forms `http.client.get(...)` or `http.server.new()`.
The declarations below are excerpted from `stdlib/http/client.tyra`
and `stdlib/http/server.tyra`.

```tyra
# stdlib/http/client.tyra
export data Response
  status: Int
  body: String
end

export type FetchError =
  | NetworkError(message: String)
  | Timeout(message: String)

export fn get(_ url: String) -> Result<Response, FetchError>
```

```tyra
# stdlib/http/server.tyra
export data Request
  method: String
  path: String
  body: String
end

export data Response
  status: Int
  body: String
end

export data AppServer
  _handle: Int
end

export fn new() -> AppServer

impl AppServerOps for AppServer
  fn get(self, _ path: String, _ handler: String) -> Unit
  fn post(self, _ path: String, _ handler: String) -> Unit
  fn listen(self, _ port: Int) -> Result<Unit, String>
end
```

**`http.client` semantics (v0.1):**

- Any 2xx / 4xx / 5xx response from a reachable server is returned as
  `Ok(Response)`; callers inspect `resp.status` to branch. `FetchError`
  only represents transport-layer failures (DNS, connection refused,
  TLS, timeout).
- `FetchError.NetworkError` is a catch-all variant; `Timeout` is the
  sole distinct variant in v0.1.
- TLS trust anchors come from Mozilla `webpki-roots` and do **not**
  consult the system CA trust store. Enterprise / private CAs are
  unsupported in v0.1.
- Response bodies are capped at 10 MiB and decoded as UTF-8. Because
  Tyra `String` is C-string-backed, payloads containing interior NUL
  bytes are truncated at the first NUL.
- Only `GET` is exposed. `POST` / `PUT` / `DELETE`, header, and query
  manipulation are deferred to later milestones.
- Each successful `get` leaks one internal response allocation for
  the lifetime of the process (v0.1 opaque-handle design, matching
  the `json` trade-off in §17.3.2). Fine for CLI / one-shot tools;
  avoid high-frequency polling loops in long-lived processes.

**`http.server` semantics (v0.1):**

- Handlers are synchronous `fn(Request) -> Response`. Failures are
  encoded as non-2xx `Response` values rather than `Result`.
- The accept loop is single-threaded and blocking; only one request
  is in flight at a time. Dispatching handlers onto the M9 task
  runtime is deferred.
- Routing is exact-path only. Wildcards and URL parameters are not
  supported. Registering the same `(method, path)` twice overwrites
  the previous handler (the runtime emits a warning).
- `Request.body` is captured raw and capped at 1 MiB. Headers,
  cookies, and query strings are not accessible from Tyra in v0.1.
- No built-in TLS. Terminate HTTPS at a reverse proxy (nginx, caddy).
- A handler that calls `panic()` aborts the whole process (§12
  abort-not-unwind semantics). Wrap risky logic in `match` /
  `Result` and return a 5xx `Response` instead.
- `listen` returns `Result<Unit, String>`, but the `Ok` arm is
  structurally unreachable in v0.1 (only bind failure returns, as
  `Err(msg)`). Pattern-match `Err(msg)` for diagnostics; `Ok(_)` is
  reserved for a future shutdown API.
- `AppServer._handle` is a GC-managed opaque handle (§8.5). See
  `runtime/src/stdlib_http.rs` and `runtime/src/stdlib_http_server.rs`
  for implementation notes.

#### 17.3.4 string

Callers `import string` and use the module-qualified form
`string.trim(...)` / `string.len(...)`. The declarations below are
excerpted from `stdlib/string.tyra`.

```tyra
# stdlib/string.tyra
export fn len(_ s: String) -> Int
export fn is_empty(_ s: String) -> Bool
export fn trim(_ s: String) -> String
export fn to_upper(_ s: String) -> String
export fn to_lower(_ s: String) -> String
export fn contains(_ s: String, _ needle: String) -> Bool
export fn starts_with(_ s: String, _ prefix: String) -> Bool
export fn ends_with(_ s: String, _ suffix: String) -> Bool
export fn parse_int(_ s: String) -> Option<Int>
export fn byte_at(_ s: String, _ index: Int) -> Option<Int>
export fn substring(_ s: String, _ start: Int, _ stop: Int) -> String
export fn reverse(_ s: String) -> String
export fn from_byte(_ b: Int) -> String
export fn split_whitespace(_ s: String) -> List<String>
export fn split(_ s: String, _ sep: String) -> List<String>
```

- `len` returns the UTF-8 byte length, not the Unicode code-point
  count (`len("あ")` is `3`). A code-point-based length is out of
  scope for v0.1.
- `trim` strips **ASCII whitespace only** from both ends (non-ASCII
  whitespace such as U+3000 is not trimmed). `to_upper` / `to_lower`
  case-map ASCII letters only; other characters pass through
  unchanged. Full Unicode support is deferred to v0.2+.
- `contains` / `starts_with` / `ends_with` perform byte-level
  substring matching.
- `parse_int` accepts an optional leading `+` / `-` followed by ASCII
  decimal digits. Leading or trailing whitespace is rejected (trim
  first if needed). Parse failure returns `None`. Radix selection is
  deferred to v0.2+.
- `byte_at(s, i)` returns the `i`-th UTF-8 **byte** as an `Int` in
  `0..=255` wrapped in `Some`, or `None` when `i` is outside
  `[0, len(s))` (negative values included). This is a byte-level
  accessor, not a code-point one (`byte_at("あ", 0)` is `Some(227)`).
- `substring(s, start, stop)` returns the byte-level half-open slice
  `[start, stop)`. Both bounds are clamped to `[0, len(s)]`, and the
  result is empty when `start >= stop`. If either index lands in the
  middle of a multi-byte UTF-8 sequence, v0.1 returns an empty string
  (a grapheme-aware API is deferred to v0.2+). The parameter is named
  `stop` because `end` is a Tyra keyword (§6).
- `reverse(s)` reverses the string byte-by-byte. The result is exact
  for ASCII inputs; multi-byte UTF-8 strings break the encoding and
  v0.1 returns an empty string (grapheme-aware reverse is v0.2+).
- `from_byte(b)` builds a single-byte string from an `Int` in
  `0..=255` (higher bits are truncated via `b & 0xFF`). Standalone
  bytes in `0x80..=0xFF` are not valid UTF-8, so v0.1 returns an empty
  string for them.
- `split_whitespace(s)` splits on runs of ASCII / Unicode whitespace
  (Rust's `char::is_whitespace`). Adjacent separators are collapsed and
  leading / trailing whitespace produce no empty entries. Empty or
  whitespace-only input returns an empty list.
- `split(s, sep)` splits on every occurrence of `sep` (byte-level,
  matching Rust's `str::split`). Adjacent separators yield empty-string
  entries. An empty `sep` is NOT split between every character in v0.1
  — the function returns the single-element list `[s]`.
- `replace` / `join` / `char_at` / regex are NOT part of this freeze.
  Tracked in §22 as "extended `string` API".

#### 17.3.5 list

Callers `import list` and use module-qualified calls like `list.push(...)`
/ `list.sum(...)`. v0.1 freezes a **`List<Int>`-only** surface of six
functions.

```tyra
# stdlib/list.tyra
export fn push(_ list: List<Int>, _ x: Int) -> List<Int>
export fn sum(_ list: List<Int>) -> Int
export fn max(_ list: List<Int>) -> Option<Int>
export fn min(_ list: List<Int>) -> Option<Int>
export fn contains(_ list: List<Int>, _ x: Int) -> Bool
export fn index_of(_ list: List<Int>, _ x: Int) -> Option<Int>
```

- All operations are **immutable**. Functions returning `List<Int>`
  (`push`) allocate a fresh GC-managed buffer and never mutate the input
  (§coding-style immutability rule).
- `push` returns a new list with the element appended at the tail, O(n).
- `sum` is a fold with identity `0`. Overflow is NOT checked in v0.1
  (follows `Int` two's-complement wrap semantics).
- `max` / `min` return `None` for the empty list and `Some(v)` otherwise.
  Ordering is the usual signed-integer comparison.
- `contains` performs a linear scan for equality.
- `index_of` returns `Some(i)` for the smallest matching index (0-based)
  or `None` if no element matches.
- Element type is **`Int` only**. `List<String>` and arbitrary `List<T>`
  are out of scope for v0.1 (requires stdlib-intrinsic element-type
  monomorphization plumbing; tracked in §22).
- `map` / `filter` / `fold` are out of scope for v0.1 (lambda passing
  through a C ABI is required). Tracked in §22 as "extended `list` API".
- Implementation emits LLVM IR directly (`__list_int_*` intrinsics →
  inline `GC_malloc` + loops); no runtime C ABI is involved. Safe because
  the `List<Int>` layout (`{ptr data, i64 len}`) is compiler-owned.

#### 17.3.6 map (v0.7.0 — HAMT persistent, ADR-0015)

`Map<K, V>` was fully generalized in v0.6.0, and reimplemented in v0.7.0 as a true
persistent data structure using HAMT (Hash Array Mapped Trie).

```tyra
let table: Map<String, Int> = {"one": 1, "two": 2}
match table.get("one")
when Some(n)
  println("got #{n}")
when None
  println("absent")
end
table.contains_key("two")   # Bool

let m: Map<Int, Bool> = {}  # bidirectional inference from expected type

# insert / remove return a new Map; original binding is unchanged
let m2 = m.insert(1, true)
let m3 = m2.remove(1)

# iteration
for k, v in table
  println("#{k}: #{v}")
end
```

- `m.get(k: K) -> Option<V>`: look up a key; returns `Some(value)` or `None`.
- `m.contains_key(k: K) -> Bool`: membership test.
- `m.len() -> Int`: entry count.
- `m.insert(k: K, v: V) -> Map<K, V>`: add a key-value pair and return a new Map. The original binding is unchanged.
- `m.remove(k: K) -> Map<K, V>`: remove a key and return a new Map. The original binding is unchanged.
- Empty literal `{}` is typed by bidirectional inference from the expected type;
  a bare `{}` with no expected type is a compile error.
- `Float` and types with `mut` fields cannot be keys (`Hash` ability unsatisfied).

**HAMT implementation notes**:
- The implementation uses HAMT (Hash Array Mapped Trie). `insert`/`remove` perform path-copy with structural sharing; the original binding is never mutated.
- Iteration order is HAMT DFS order (hash-based), not insertion order.
- `insert`/`remove` during iteration only return new bindings; the in-progress iteration is unaffected.

**Later releases**: user-defined `value` types as keys (Eq + Hash codegen for structs), map merge/diff operations.

#### 17.3.7 set (v0.7.0 — HAMT persistent, ADR-0015)

`Set<T>` was added in v0.6.0, and reimplemented in v0.7.0 as a true persistent data
structure using HAMT (Hash Array Mapped Trie).

```tyra
import set

let s: Set<Int> = set.new()  # explicit annotation required at construction
let s = s.insert(1)           # returns a new Set<Int>; idempotent
let s = s.insert(2)
let s = s.insert(1)           # duplicate — len unchanged
s.contains(2)                 # Bool: true
s.len()                       # Int: 2

# remove returns a new Set; original binding is unchanged
let s2 = s.remove(1)

# iteration
for v in s
  println("#{v}")
end
```

- `set.new() -> Set<T>`: create an empty set. `T` must be inferable from context,
  or supply an explicit annotation (`let s: Set<Int> = set.new()`).
- `s.insert(v: T) -> Set<T>`: add an element and return the updated set (idempotent). The original binding is unchanged.
- `s.remove(v: T) -> Set<T>`: remove an element and return a new set. The original binding is unchanged.
- `s.contains(v: T) -> Bool`: membership test.
- `s.len() -> Int`: element count.
- No set-literal syntax (`{}` conflicts with map literals; use `set.new()` + `.insert()`).
- `Float` and types with `mut` fields cannot be elements (`Hash` ability unsatisfied).

**HAMT implementation notes**:
- The implementation uses HAMT (Hash Array Mapped Trie). `insert`/`remove` perform path-copy with structural sharing; the original binding is never mutated.
- Iteration order is HAMT DFS order (hash-based), not insertion order.
- `insert`/`remove` during iteration only return new bindings; the in-progress iteration is unaffected.

**Later releases**: set operations (union/intersection/diff), user-defined value types as elements.

#### 17.3.8 time (v0.6.0)

```tyra
import time

let unix = time.now_unix()          # Int — seconds since Unix epoch
let ms   = time.monotonic_millis()  # Int — milliseconds since process start
```

- `time.now_unix() -> Int`: wall-clock Unix timestamp (seconds).
- `time.monotonic_millis() -> Int`: monotonic elapsed milliseconds; suitable for
  measuring durations, not wall time.

#### 17.3.9 log (v0.6.0)

```tyra
import log

log.info("server started")
log.warn("retry #{n}")
log.error("connection refused: #{addr}")
```

- `log.info(_ msg: String) -> Unit`: informational message → **stderr**.
- `log.warn(_ msg: String) -> Unit`: warning → **stderr**.
- `log.error(_ msg: String) -> Unit`: error → **stderr**.
- All three functions write to **stderr** (not stdout). No timestamps or
  structured fields are added in v0.6.0.

---

## 18. Toolchain

Tyra integrates all development operations into a single official CLI. No separate tool installation is required.

```bash
tyra check   tyra run    tyra build  tyra fmt
tyra test    tyra new    tyra mod    tyra bench
```

### 18.1 tyra check

Type-checks a source file without compiling.

```bash
tyra check                    # project mode: auto-discovers entry point via Tyra.toml
tyra check src/myapp.tyra    # specify file directly
```

- No errors → exit 0; errors → exit 1
- Project mode: walks up from the current directory to find `Tyra.toml`, then checks `src/<name>.tyra`

### 18.2 tyra run

Compiles and runs in one step. No binary is written to disk.

```bash
tyra run                           # project mode
tyra run src/myapp.tyra            # specify file directly
tyra run --release src/myapp.tyra  # run with optimized build (-O2)
```

### 18.3 tyra build

Compiles to a native binary.

```bash
tyra build                         # project mode: output to <project_root>/<name>
tyra build --release               # optimized build (-O2)
tyra build -o dist/myapp           # explicit output path
tyra build src/myapp.tyra -o out   # specify file and output path directly
```

- Debug build (default): `-O0`; `--release`: `-O2`
- In project mode the output is placed at the project root, not inside `src/`

### 18.4 tyra fmt

Formats Tyra source to the canonical style.

```bash
tyra fmt src/myapp.tyra           # format a file in-place
tyra fmt src/                     # recursively format a directory
tyra fmt --check src/             # list files that would change and exit 1 (CI-friendly)
tyra fmt --stdin                  # read from stdin, write formatted source to stdout
```

- Indentation: 2 spaces
- Line limit: 100 columns; argument lists that exceed this wrap one-param-per-line (idempotent)
- Comments (standalone and inline) are preserved in their original positions

### 18.5 tyra test

Discovers and runs tests automatically.

```bash
tyra test                          # run all *_test.tyra files under the current directory
tyra test src/                     # specify a directory
tyra test math_test.tyra           # specify a single file
tyra test --filter <pattern>       # substring match on test function names
tyra test --list                   # list matched functions without running
tyra test --format tap             # TAP version 14 (default)
tyra test --format junit           # JUnit-compatible XML (for CI test summaries)
```

- Target files: `*_test.tyra`
- Test functions: `fn test_*() -> Result<Unit, String>` (no parameters)
- TAP output includes `# time: <s>s` at the end of each file's run
- JUnit: if a file fails to compile, a synthetic single-test suite is emitted to prevent silent green in CI
- Each `<testsuite>` carries a `time=` attribute
- **E0216**: `*_test.tyra` files must not contain `fn main` or top-level executable statements

```tyra
# example_test.tyra
import assert

fn test_add() -> Result<Unit, String>
  assert.eq(1 + 1, 2)?
  Ok(())
end
```

### 18.6 tyra new

Scaffolds a new project.

```bash
tyra new myapp              # bin project (src/myapp.tyra, Tyra.toml, .gitignore, README.md)
tyra new mylib --lib        # lib project (src/mylib.tyra with export fn)
tyra new myapp --vcs none   # suppress .gitignore (for sub-projects inside an existing repo)
```

- `src/<name>.tyra` filename must match the package name (§13.1 invariant)
- Bin packages (containing `fn main` or top-level statements) cannot be imported (E0218)
- Lib packages consist of declarations only; symbols are published with `export fn`

### 18.7 tyra mod

Manages package dependencies. Operates from any directory containing a `Tyra.toml`.

```bash
tyra mod init [--name <n>]                      # create Tyra.toml in an existing directory
tyra mod add <name> --path <path>               # add a path dependency
tyra mod add <name> --git <url> --rev <sha>     # add a git dependency (rev guarantees reproducibility)
tyra mod update <name> --path <path>            # update an existing entry in-place
tyra mod update <name> --git <url> --rev <sha>  # update a git dependency's rev
tyra mod remove <name>                          # delete a dependency entry
tyra mod show <name> [--json]                   # print dependency details
tyra mod tree [--json]                          # print the dependency tree (cycle detection, DAG-safe)
tyra mod sync [--check] [--json] [--quiet]      # clone git deps; --check validates without mutating
tyra mod clean                                  # remove ~/.tyra/cache/
```

**Import resolution order (ADR 0010)**: local `src/` → `[dependencies]` → stdlib, uniqueness rule. If the same module name appears in two or more layers, E0217 is emitted (ambiguity error). Silent shadowing is never performed.

**Dependency invariants (ADR 0009)**:
- The dep key must equal the `package.name` declared in the target `Tyra.toml` (no aliasing)
- Bin packages cannot be imported as dependencies (E0218)
- A dependency with no `src/<name>.tyra` is rejected at `tyra mod sync` time

### 18.8 tyra bench

Runs benchmarks.

```bash
tyra bench ai-gen [options]   # AI-generation quality benchmark (delegates to bench/ai-gen/harness.py)
```

- Forwards `--languages`, `--generators`, `--prompts`, `--seed`, `--dry-run`, `--inject-tyra-spec`, `--results-dir` verbatim to harness.py
- General-purpose microbenchmark runner (`tyra bench <dir>`) is planned for v0.4.0

### 18.9 Goals

- Reproduce Go-style operational simplicity: no per-language tool installation required
- Minimize learning cost: all development operations complete with a single `tyra` command
- Reduce in-team option proliferation: formatter, test runner, and package manager are all official

### 18.10 Build artifacts

- Default (debug) build: no optimization (`-O0`)
- `--release` enables `-O2` optimization
- In project mode, output is placed at the project root (`-o` overrides)
- Targets: macOS arm64 / Linux x86_64 (cross-compilation not yet supported)

---

## 19. Execution model

### 19.1 Entry point

Program execution starts from `fn main`. The entry-point file takes one of the following forms:

1. Explicit `fn main` — one of `fn main() -> Unit`, `fn main() -> Result<Unit, E>`, or `async fn main() -> Result<Unit, E>`
2. Top-level executable statements — normalized to an implicit `fn main() -> Unit` (§6.1)

`fn main` may only be defined in the entry-point file and may not have `export`. Application packages require exactly one entry point. Library packages do not require an entry point. A compile error occurs if multiple entry points are detected.

### 19.2 Compilation pipeline

```text
source -> lexer -> parser -> typed AST -> mid-level IR -> backend IR -> native binary
```

Reference implementation:

```text
source -> lexer -> parser -> typed AST -> mid-level IR -> LLVM IR -> native binary
```

For files with top-level executable statements, the frontend classifies declarations and executable statements, then normalizes the executable statements into an implicit `fn main() -> Unit`. The normalized AST is identical to an explicit main, and subsequent phases are unaffected.

### 19.3 Implementation policy

- The parser is simplified by assuming syntax with little ambiguity
- The IR after type checking has a stable shape for AI and tooling
- WASM as a future target is under consideration but is not in scope for v0.1

---

## 20. Formatting rules

The formatter enforces:

- 2-space indentation
- Unified trailing-comma rules
- Fixed layout for `match` and `if`
- Fixed import order
- Line breaks are decided by the formatter at syntactic units

Free style choices are not allowed.

---

## 21. Examples

### 21.0 Minimal program

With top-level executable statements (§6.1), the minimal program is a single line:

```tyra
print("hello, tyra")
```

Function definitions and top-level executable statements can be mixed. Top-level declarations support forward references, so a function defined after an executable statement can still be called:

```tyra
print("fib(10) = #{fib(10)}")

fn fib(_ n: Int) -> Int
  match n
  when 0
    0
  when 1
    1
  when _
    fib(n - 1) + fib(n - 2)
  end
end
```

When error propagation or async is needed, use an explicit `fn main`:

```tyra
fn main() -> Result<Unit, AppError>
  let config = read_config("app.conf")?
  start_server(config)?
end
```

### 21.1 ADT and match

```tyra
type Payment =
  | Card(last4: String)
  | Bank(bank_name: String)
  | Cash

fn label(payment: Payment) -> String
  match payment
  when Card(last4: last4)
    "card: #{last4}"
  when Bank(bank_name: bank_name)
    "bank: #{bank_name}"
  when Cash
    "cash"
  end
end
```

### 21.2 Result propagation

```tyra
fn read_port() -> Result<Int, ConfigError>
  let text = fs.read_to_string("app.conf")?
  parse_int(text)?
end
```

### 21.3 Entry point of an HTTP server

```tyra
import http.server

async fn main() -> Result<Unit, AppError>
  let app = server.new()
  app.get("/health", health_handler)
  app.listen(port: 8080).await?
end
```

---

## 22. Deferred items

The following are postponed for later specification:

- macro system
- operator overloading
- language-level actor model
- centralized vs distributed package registry policy
- foreign function interface (FFI) details
- task cancellation
- multi-line string
- 3 or more constraints, `where` clauses, associated types
- guard clauses (`when pattern if condition`)
- tuple types
- structured concurrency
- Module-level initialization semantics (`let`/`mut` at module scope)
- Extended `string` API (replace, join, char_at, regex) — `split` and `split_whitespace` are frozen in §17.3.4; everything else is a later release
- Extended `list` API — generic `List<T>`, `map` / `filter` / `fold`, and `List<String>` are implemented in v0.4.0 (§17.3.5). `sort_by` and other additional operations are a later release
- `Map<K,V>` — HAMT persistent in v0.7.0 (§17.3.6). `remove` / `for k, v in m` iteration implemented. User-defined `value` type keys and merge/diff operations are a later release
- `Set<T>` — HAMT persistent in v0.7.0 (§17.3.7). `remove` / `for v in s` iteration implemented. Set-literal syntax and `union`/`intersection` are a later release
- `test "name"` language syntax — implemented in v0.6.0 (ADR-0013)
- `assert.panics` — implemented as runner-native panic expectation in v0.6.0 (ADR-0012); callable stdlib API is not provided
- Generic `assert.eq<T>` — overloads for `Int` / `String` / `Bool` are implemented in v0.4.0; full generics via ability constraints are a later release

---

## 23. Summary of Tyra v0.1

Tyra v0.1 is a language with the following character:

- Readable syntax inspired by Ruby
- Less ambiguity than Python
- Practical types in the spirit of TypeScript
- Less strict than Rust
- Simple build / test / fmt / deploy like Go
- Consistent syntax and conventions for AI-assisted completion

Tyra is a practical language that prioritizes **readability, type safety, ease of distribution, and predictability** above all.
