# Tyra Language Specification

- **Version**: 0.1 (under development)
- **Status**: Draft
- **Last updated**: 2026-04-15

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

`fn`, `data`, `value`, `type`, `trait`, `impl`, `let`, `mut`, `if`, `else`, `match`, `when`, `for`, `in`, `while`, `return`, `defer`, `async`, `await`, `spawn`, `import`, `export`, `or`, `true`, `false`, `end`

### 5.3 Comments

```tyra
# line comment
```

Multi-line comments are not adopted in v0.1.

### 5.4 Statement termination

- Statements are separated by newlines
- `,` is used only when necessary
- `;` is not adopted

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

`Int` is a 64-bit signed integer; `Float` is IEEE 754 double precision.
Integer literals default to `Int` and floating-point literals default to `Float` when no contextual type is available.
`Rune` is a 32-bit value representing a Unicode scalar value. Grapheme clusters are the responsibility of `String`.

### 7.3 Strings

Regular strings support interpolation.

```tyra
let msg = "hello, #{user.name}"
```

Raw strings and multi-line strings are not standardized in v0.1. Multi-line text should be expressed by concatenating regular strings.

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

Rules:

- Each type parameter may have 0 or 1 constraints
- v0.1 allows only the simple form `<T: Constraint>`
- `Constraint` may be either a trait or an ability
- `where` clauses, multiple constraints, associated types, and higher-kinded types are not adopted
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

- `match` must be exhaustive
- Tagged unions are the standard
- Constructor patterns with named fields use named destructuring by default
- `when Card(last4)` is shorthand for `when Card(last4: last4)`

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
- Automatically gains the `Hash` ability if all fields satisfy `Hash`
- The `Ord` ability is **not** automatically derived
- Automatically gains the `Debug` ability if all fields satisfy `Debug`
- A type with the `Hash` ability necessarily has the `Eq` ability
- `==` is available when the type has the `Eq` ability
- When ordering is needed, use key-function APIs such as `sort_by`, `min_by`, and `max_by`

```tyra
data User
  id: Int
  mut name: String
end
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
- The value type of every `when` arm must match
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

---

## 11. Collections

Standard collections:

- `List<T>`
- `Map<K, V>`
- `Set<T>`

Literals:

```tyra
let nums = [1, 2, 3]
let user_by_id = {1: "mika", 2: "jun"}
```

- Map literal keys may be arbitrary expressions
- The key type `K` of `Map<K, V>` must satisfy `Hash`
- Indexing uses `items[index]`

---

## 12. Error handling

### 12.1 Principles

- Predictable failures use `Result`
- Unpredictable bugs cause panic
- Exception mechanisms are not adopted in v0.1
- `Option` represents absence and is separated from error-propagation syntax

### 12.2 Propagation operator

`?` is exclusive to `Result`.

```tyra
fn load_user(_ id: Int) -> Result<User, AppError>
  let row = db.find(id)?
  decode_user(row)?
end
```

Rules:

- `expr?` is usable only when `expr` has type `Result<T, E>`
- The enclosing function's return type must be `Result<U, F>`
- `E` must implement `Into<F>`
- `?` cannot be applied to `Option<T>`

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

For early return on `Option`, use `or return`.

```tyra
fn user_name(_ id: Int) -> Option<String>
  let user = repo.find(id) or return None
  Some(user.name)
end
```

Rules:

- The left-hand side of `or return` must have type `Option<T>`
- `expr or return e` evaluates to `value` when `expr` is `Some(value)`
- When `expr` is `None`, the function returns `e` and exits
- The type of `e` must match the enclosing function's return type
- The enclosing function's return type is unconstrained (may be `Option`, `Result`, or anything else)
- `Bool`, `Result`, and other types cannot be placed on the left-hand side

Examples:

```tyra
# Return type Option<T>
fn lookup_name(_ id: Int) -> Option<String>
  let user = repo.find(id) or return None
  Some(user.name)
end

# Return type Result<T, E>
fn require_user(_ id: Int) -> Result<User, AppError>
  let user = repo.find(id) or return Err(AppError.NotFound)
  Ok(user)
end

# Return type Bool
fn user_exists(_ id: Int) -> Bool
  let _user = repo.find(id) or return false
  true
end
```

### 12.3 defer

```tyra
fn handle() -> Result<Unit, AppError>
  let file = fs.open("app.log")?
  defer file.close()
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
let task = spawn sync_cache()
let result = task.await
```

Rules:

- `spawn expr` runs the arbitrary expression `expr` concurrently and returns `Task<T>`
- If `expr` is an async function call, the runtime performs the equivalent of `.await` internally and wraps the final result in `Task<T>`
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

Included in v0.1:

- `core`
- `string`
- `collections`
- `option`
- `result`
- `json`
- `http`
- `fs`
- `time`
- `test`
- `log`

Standard traits auto-imported in the prelude:

- `Into<T>`
- `Stringable`

Compiler-known standard abilities:

- `Eq`
- `Hash`
- `Ord`
- `Debug`

Operator correspondences:

- `==`, `!=` -> `Eq`
- `<`, `<=`, `>`, `>=` -> `Ord`
- `+`, `-`, `*`, `/` -> built-in numeric operations only; no operator overloading

Standard APIs related to collections and ordering:

- `List.sort_by(fn(T) -> K)`
- `List.min_by(fn(T) -> K)`
- `List.max_by(fn(T) -> K)`

Principles:

- Things commonly used in production are in the standard library
- Reproducibility is preferred over freedom of dependency choice

---

## 18. Toolchain

Tyra unifies the official CLI into a single binary.

```bash
tyra new app
tyra run
tyra build
tyra test
tyra fmt
tyra mod
```

### 18.1 Goals

- Reproduce Go-style operational simplicity
- Reduce learning cost
- Reduce in-team option proliferation

### 18.2 Build artifacts

- The default output is a single native binary
- Both release and debug builds are supported

---

## 19. Execution model

Compilation pipeline:

```text
source -> lexer -> parser -> typed AST -> mid-level IR -> backend IR -> native binary
```

Reference implementation:

```text
source -> lexer -> parser -> typed AST -> mid-level IR -> LLVM IR -> native binary
```

### 19.1 Implementation policy

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

export async fn main() -> Result<Unit, AppError>
  let app = server.new()
  app.get("/health", health_handler)
  app.listen(port: 8080).await?
end
```

---

## 22. Items deferred in v0.1

The following are postponed for later specification:

- macro system
- operator overloading
- language-level actor model
- centralized vs distributed package registry policy
- foreign function interface (FFI) details
- task cancellation
- raw strings / multi-line strings
- multiple constraints, `where` clauses, associated types

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
