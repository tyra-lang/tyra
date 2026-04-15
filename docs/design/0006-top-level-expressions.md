# ADR 0006: Allow top-level expressions as implicit main

- **Status**: Accepted
- **Date**: 2026-04-15
- **Spec sections affected**: §6, §9.1, §13.1, §19

## Context

Tyra v0.1 requires an explicit `fn main` as the program entry point:

```tyra
fn main() -> Unit
  print("hello, tyra")
end
```

This design follows Go's model and aligns with Tyra's explicitness principle (§2.1). However, during Phase 0b (competitive analysis), this was identified as a friction point against every competitor:

| Language | Hello world | Lines |
| -- | -- | -- |
| Ruby     | `puts "hello"` | 1 |
| Crystal  | `puts "hello"` | 1 |
| Python   | `print("hello")` | 1 |
| Tyra     | `fn main() -> Unit` + `print(...)` + `end` | 3 |

Crystal is Tyra's closest competitor (Ruby syntax + static types + LLVM). Crystal allows top-level code. This 3x verbosity gap hurts Tyra's first impression and contradicts the "Ruby-derived readability" narrative (§2.2).

The question is whether top-level expressions can be permitted without violating Tyra's design principles.

## Decision

**Allow top-level executable statements in entry-point files. They are desugared to `fn main() -> Unit`.**

### Rules

#### Rule 1: Entry-point files may contain top-level executable statements

These are desugared to an implicit `fn main() -> Unit` whose body consists of the top-level executable statements in source order.

#### Rule 2: `fn main` and top-level executable statements are mutually exclusive

A file that defines `fn main` must not contain top-level executable statements. A file that contains top-level executable statements must not define `fn main`. Violation is a compile error.

#### Rule 3: `?` is not allowed in top-level executable statements

The implicit main returns `Unit`, which is neither `Result` nor `Option`. Programs that need error propagation at the entry point must use explicit `fn main() -> Result<Unit, E>`.

#### Rule 4: `.await` is not allowed in top-level executable statements

The implicit main is a synchronous function. Programs that need async operations at the entry point must use explicit `async fn main() -> Result<Unit, E>`.

Rationale: permitting `.await` at the top level would require the implicit main to be `async fn main()`, introducing implicit async context. Tyra's design requires suspension points to be explicit (§14). If async is needed, write `async fn main`.

#### Rule 5: `return` is not allowed in top-level executable statements

Although the implicit main is technically a function, top-level `return` is confusing to read and explain. Programs that need early exit should use explicit `fn main` or call `exit()`.

#### Rule 6: Module files may not contain top-level executable statements

Exactly one entry-point file must be designated by the toolchain. The file passed to `tyra run` or designated as the entry point by `tyra build` is the entry-point file. Compilation fails if zero or multiple entry points are detected (e.g., two files each containing `fn main`, or a file with `fn main` importing a file with top-level executable statements).

Only the entry-point file may have top-level executable statements. Files that are `import`-ed may only contain declarations.

v0.1 does not define module-level initialization semantics. Module files may not contain top-level `let` or `mut` bindings. This restriction may be relaxed in a future version once module initialization order is specified.

#### Rule 7: Top-level `let`/`mut` in entry-point files are implicit main locals

In an entry-point file with top-level executable statements, `let` and `mut` bindings are local to the implicit main — not module-scope bindings. They follow the same scoping rules as bindings inside an explicit `fn main`.

```tyra
let x = 42        # local to implicit main, not a module-level binding
print("#{x}")
```

This avoids introducing module-level mutable state or initialization-order semantics in v0.1.

#### Rule 8: Permitted top-level executable forms

The following are permitted as top-level executable statements in entry-point files. They correspond to the statements permitted inside `fn main`:

| Category | Examples | Permitted |
| -- | -- | -- |
| Expression statement | `print("hello")`, `run()` | Yes |
| Local binding | `let x = 1`, `mut count = 0` | Yes |
| `if` / `else if` / `else` | `if ready ... end` | Yes |
| `match` | `match x ... end` | Yes |
| `for` | `for item in items ... end` | Yes |
| `while` | `while running ... end` | Yes |
| `defer` | `defer file.close()` | Yes (see note) |
| `return` | `return` | **No** (Rule 5) |
| `?` | `expr?` | **No** (Rule 3) |
| `.await` | `expr.await` | **No** (Rule 4) |

Declarations (`fn`, `type`, `value`, `data`, `trait`, `impl`, `import`, `export`) are always allowed in all files. They are not executable statements and are not part of the implicit main body.

**Note on `defer`**: In top-level executable statements, `defer` executes at the end of the implicit main (i.e., program termination), not at intermediate block boundaries within the top-level code. This follows from the desugaring: all top-level statements are the body of a single `fn main`, so `defer` obeys the standard LIFO scope-exit rule of that function.

### Desugaring example

Source:

```tyra
fn greet(_ name: String) -> String
  "hello, #{name}"
end

let msg = greet("tyra")
print(msg)
```

Desugars to:

```tyra
fn greet(_ name: String) -> String
  "hello, #{name}"
end

fn main() -> Unit
  let msg = greet("tyra")
  print(msg)
end
```

The desugaring is purely syntactic. The compiler collects all top-level executable statements (excluding declarations) and wraps them in `fn main() -> Unit ... end`. Declarations remain at module scope.

### Progression model

The design creates a natural learning path:

```text
Level 1: print("hello")                          # top-level, no main
Level 2: fn main() -> Unit ... end                # explicit main, no errors
Level 3: fn main() -> Result<Unit, E> ... end     # explicit main, error handling
Level 4: async fn main() -> Result<Unit, E>       # async + error handling
```

Each level adds exactly one concept. Top-level mode covers Level 1 only. The moment a program needs error propagation or async, it graduates to explicit main.

### When to use which style

| Scenario | Style | Reason |
| -- | -- | -- |
| Hello world, simple script | Top-level | Minimal ceremony |
| Application with error handling | `fn main() -> Result<Unit, E>` | `?` requires Result return |
| Async entry point | `async fn main() -> Result<Unit, E>` | `.await` requires async |
| Library / module | No main, no top-level | Declarations only |

## Consequences

### What becomes easier

- **First impression**: `print("hello, tyra")` is a valid Tyra program — competitive with Ruby, Crystal, and Python.
- **Scripting use cases**: Small CLI tools, one-off scripts, and examples require less boilerplate.
- **Teaching**: Beginners can start writing Tyra without learning about `fn`, `->`, return types, or `end`. These concepts are introduced when programs grow complex enough to need them.
- **Crystal comparison**: The "but Crystal is a one-liner" argument is eliminated.
- **AI code generation**: For simple prompts ("write hello world in Tyra"), AI generates shorter, correct code. The heuristic is simple: if the task needs `Result` or `async`, generate explicit main; otherwise, generate top-level.

### What becomes harder

- **Two file structures**: Entry-point files have two valid shapes (top-level or explicit main). This is mitigated by the mutual exclusion rule — each file is unambiguously one or the other.
- **Top-level `?` is a compile error**: Users will discover this when they try to propagate errors at the top level. The error message must guide them to explicit `fn main() -> Result<Unit, E>`. This is a learning step, but it naturally teaches the Result-based error handling pattern.
- **Module boundary enforcement**: The compiler must track which file is the entry point and reject top-level executable statements in non-entry-point files. This is a minor implementation cost.
- **Top-level `let`/`mut` are NOT module constants**: Users who expect Python-like module-level globals will be surprised. The error message should explain that `let`/`mut` are local to the implicit main, and module-level bindings are not yet supported.

### Impact on AI-friendliness (§2.5)

The desugaring rule is deterministic and simple:

- **Parsing**: The parser can distinguish declarations from executable statements syntactically. No ambiguity.
- **AST**: The AST always has a `main` function. Whether it was explicit or implicit is metadata, not a structural difference.
- **AI generation**: A simple heuristic applies: if the task needs `Result` return or `async`, generate explicit main; otherwise, generate top-level code. This should be encoded in AGENTS.md.

### Impact on the spec

- §6 (Block syntax): Add a note that top-level executable statements in entry-point files are wrapped in an implicit `fn main() -> Unit` block.
- §9.1 (Function definition): Add a note that `fn main` is optional for entry-point files.
- §13.1 (File and module): Clarify that module files (imported files) may only contain declarations. Top-level `let`/`mut` are prohibited in module files in v0.1.
- §19 (Execution model): Clarify the entry-point file designation and the desugaring step.

### Impact on the formatter (§20)

The formatter should handle both styles consistently. No style preference is enforced — both are valid, and the formatter does not convert between them.

### Impact on the toolchain (§18)

`tyra run file.tyra` designates `file.tyra` as the entry point. `tyra build` uses the project configuration to determine the entry point. No change to the CLI interface.

### Impact on AGENTS.md

AGENTS.md should include guidance for AI assistants:

```markdown
### Entry-point style guidance

- Trivial examples and one-off scripts: use top-level style (no fn main)
- Production apps, async apps, error-propagating entry points: use explicit fn main
- Never mix both in one file
- When in doubt, use explicit fn main (it is always valid)
```

## Alternatives considered

### A. Keep the status quo: `fn main` always required

Continue requiring `fn main` in all programs.

**Rejected** because:

- 3x verbosity for hello world compared to every competitor (Ruby, Crystal, Python, Go is 5 lines but has cultural acceptance)
- Contradicts §2.2 (readability) for simple programs
- The Phase 0b competitive analysis showed this is the single most visible weakness of Tyra's syntax
- Crystal, the closest competitor, allows top-level code with no reported problems

### B. Allow top-level `?` with implicit error handling

Top-level `?` would print the error to stderr and call `exit(1)`.

```tyra
# Would desugar to: match expr { Ok(v) -> v, Err(e) -> { eprintln(e); exit(1) } }
let config = read_config("app.conf")?
```

**Rejected** because:

- Implicit behavior (printing + exiting) violates §2.1 (explicitness)
- The error output format is unspecified — should it use `Debug`? A custom format?
- Users cannot customize the error handling behavior without switching to explicit main
- The "surprise" factor when top-level `?` behaves differently from in-function `?` is high
- If users need error propagation, they should use explicit `fn main() -> Result<Unit, E>` — this is a feature, not a limitation, because it teaches the Result pattern

### C. Use a file-level annotation for the implicit main's return type

```tyra
#! -> Result<Unit, AppError>

let config = read_config("app.conf")?
```

**Rejected** because:

- Novel syntax with no precedent in Tyra's design
- A comment-like directive (`#!`) controlling type semantics is confusing
- If you need a typed main, you already have `fn main() -> Result<Unit, E>` — this adds no value
- Increases parser complexity for marginal ergonomic benefit

### D. Allow both `fn main` and top-level expressions in the same file

Top-level expressions would run before `fn main`.

**Rejected** because:

- Directly violates §2.1 (explicitness) — two entry paths in one file
- Execution order becomes ambiguous: do top-level expressions run before or after `fn main`?
- Creates a category of bugs where initialization code runs unexpectedly
- No precedent in any mainstream language

### E. Use a different file extension for script mode

Like Kotlin's `.kt` (needs main) vs. `.kts` (script mode).

**Rejected** because:

- Fragments the ecosystem — tooling, editors, and formatters must handle two file types
- Violates §2.4 (operational simplicity)
- The mutual exclusion rule achieves the same clarity without a new file extension
- Crystal, Python, and Ruby all use a single file extension with no problems

### F. Allow top-level `.await` by making implicit main async

The implicit main would be `async fn main() -> Unit` if any `.await` appears.

**Rejected** because:

- The async-ness of main is now determined by usage, not declaration — violates §2.1
- Whether the runtime starts an async scheduler becomes implicit
- Debugging "why is my program spawning an event loop" requires understanding a hidden rule
- The progression model (top-level → explicit main → async main) is cleaner and more teachable

### G. Allow top-level `return` as early exit

Top-level `return` would exit the implicit main.

**Rejected** because:

- `return` in top-level code looks like "return from the file/module", which is confusing
- The explanation "it returns from the implicit main you can't see" is hard to teach
- `exit(0)` or `exit(1)` is available for early program termination and is more explicit
- Removing this special case keeps the mental model simple: top-level code runs top to bottom

## References

- Phase 0b competitive analysis: `comparisons/ANALYSIS.md`
- Crystal top-level code: <https://crystal-lang.org/reference/syntax_and_semantics/the_program.html>
- Python top-level code: module-level code is the entry point (PEP 299 rejected explicit main)
- Ruby top-level code: all code outside class/def is executed
- Kotlin script mode: <https://kotlinlang.org/docs/custom-script-deps-tutorial.html>
- Go's `func main`: <https://go.dev/doc/tutorial/getting-started>
