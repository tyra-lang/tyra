# Tyra

A statically-typed, AI-friendly programming language for backend services, CLI tools, and business applications.

> **v0.3.0** — adds `tyra new`, `tyra mod` (project lifecycle), `tyra bench ai-gen`, `tyra test --filter`, and `tyra fmt` line-length wrapping. [See known limitations](#known-limitations) before using in production.

---

## What is Tyra?

Tyra is a general-purpose language designed from the ground up for an era where humans and LLMs collaborate on code. Every design decision prioritizes **interpretive consistency**: the same input should yield the same parse, the same type, the same meaning — for both humans and AI.

```tyra
import fs

fn word_count(path: String) -> Result<Int, fs.Error>
  let text = fs.read_to_string(path)?
  Ok(text.split(" ").length())
end

export fn main() -> Unit
  match word_count("notes.txt")
  when Ok(n)
    print("#{n} words")
  when Err(e)
    print("error: #{e}")
  end
end
```

## Why another language?

Existing languages are optimized for humans alone. Tyra asks: *what would a language look like if it were designed for human-AI collaboration from day one?*

The answer is a language that:

- **Has no `null`, no truthy/falsy, no implicit conversions** — ambiguity is the enemy of both humans and LLMs
- **Requires explicit argument labels** at call sites, like Swift — so reading code never requires looking up the function definition
- **Distinguishes value types and reference types** at the language level — so memory semantics are visible, not inferred
- **Separates traits (replaceable behaviors) from abilities (structural properties)** — a novel design that prevents the trait/derive boilerplate of Rust
- **Uses `end` blocks, not braces** — so block boundaries are unambiguous in any visual context
- **Has one official toolchain**: `check`, `run`, `build`, `fmt`, `test`, `new`, and `mod` — all in a single CLI; no separate package manager to install

## Design influences

Tyra borrows selectively, not wholesale, from existing languages:

| From | What |
| --- | --- |
| Swift | Argument labels, value/reference distinction, `Optional` philosophy |
| Rust | `Result<T, E>`, `?` operator, ADTs with exhaustive `match`, traits |
| Ruby | `end` blocks, string interpolation `#{...}` |
| Go | Unified toolchain, GC, single-binary distribution |
| Kotlin | The data class spirit, applied to value types |

The combination, and especially the **trait/ability separation**, is original to Tyra.

## Hello, World

```tyra
export fn main() -> Unit
  print("hello, tyra")
end
```

## A taste of the type system

```tyra
# Algebraic data types with exhaustive pattern matching
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

# Errors as values, propagated with ?
fn read_port() -> Result<Int, ConfigError>
  let text = fs.read_to_string("app.conf")?
  parse_int(text)?
end

# Value types with auto-derived equality
value Point
  x: Float
  y: Float
end

let p1 = Point(x: 1.0, y: 2.0)
let p2 = p1.copy(x: 3.0)
```

## Quick start: testing

Create a `*_test.tyra` file and run `tyra test`:

```tyra
# math_test.tyra
import assert

fn test_add() -> Result<Unit, String>
  assert.eq(1 + 1, 2)?
  Ok(())
end
```

```bash
tyra test                      # run all *_test.tyra files in the current directory
tyra test src/                 # run a specific directory
tyra test --filter add         # run only tests whose name contains "add"
tyra test --list               # list test functions without running
tyra test --format junit       # emit JUnit XML (for CI test summaries)
```

See [docs/getting-started/08-testing.md](docs/getting-started/08-testing.md) for the full guide.

## Status

**Stable in v0.3.0** — supported and tested:

| Component | Notes |
| --- | --- |
| Language specification v0.3 | ✅ Complete |
| Lexer, Parser, Type checker | ✅ Complete |
| LLVM codegen + Boehm GC runtime | ✅ macOS arm64 / Linux x86_64 |
| Standard library: string, list, fs, io, float, json, assert | ✅ Complete |
| `tyra check / run / build` CLI (zero-arg project mode, `--release`) | ✅ Complete |
| `tyra fmt [--check] [--stdin] <file\|dir>` — formatter + 100-col wrapping | ✅ Complete |
| `tyra test [--filter <pat>] [--list] [--format tap\|junit] [path]` | ✅ Complete |
| `continue` statement | ✅ Complete |
| `tyra new <name> [--lib] [--vcs none]` — project scaffolding | ✅ Complete |
| `tyra mod init/add/update/remove/show/tree/sync/clean` — dependency management | ✅ Complete |
| `tyra bench ai-gen` — AI generation benchmark runner | ✅ Complete |
| LSP server (`tyra-lsp`) + VS Code extension | ✅ Development install |
| Static conformance corpus (14 programs + error cases) | ✅ CI-gated |

**Experimental in v0.3.0** — included but not production-ready:

| Component | Notes |
| --- | --- |
| `http.server` stdlib | ⚠️ Basic GET/POST routing only; not production-ready |

**Not in v0.3.0** — explicit backlog:

| Component | Notes |
| --- | --- |
| SemVer resolver, `Tyra.lock` | ⏳ v0.5+ |
| Registry (`tyra publish`) | ⏳ v0.5+ |
| Lambda C ABI, generic `List<T>` | ⏳ v0.4.0 |
| `assert.panics` | ⏳ Requires per-test process isolation |
| `test "name"` language syntax | ⏳ Separate ADR |
| Pre-built binaries (homebrew, apt) | ⏳ Later |
| VS Code Marketplace publication | ⏳ Later |

## Known limitations

- **Windows**: untested. Build via WSL2 is recommended.
- **`http.server`**: experimental. Single-threaded, no TLS, no middleware. Do not use in production.
- **Breaking changes**: expect breaking changes before v1.0.

## Documentation

- **[Getting Started](docs/getting-started/README.md)** — installation, hello world, testing, and project lifecycle
  - [Project Lifecycle](docs/getting-started/09-project-lifecycle.md) — `tyra new`, `tyra mod`, dependencies, builds
- **[Language Specification (Japanese)](docs/spec/ja/language-spec.md)** — the authoritative source of truth
- **[Language Specification (English)](docs/spec/en/language-spec.md)** — translation, may lag behind
- **[Design Decisions](docs/design/)** — architecture decision records explaining *why*
- **[RFCs](docs/rfcs/)** — proposed changes for future versions

## Goals

Tyra is designed for:

- Web backends and API servers
- CLI tools
- Internal business applications
- Small to medium-scale services

Tyra is **not** designed for:

- Operating systems or kernels
- Frontend (browser) development
- Embedded systems with extreme resource constraints
- Replacing Rust where its borrow checker is needed

## Non-goals

To keep the language small and predictable:

- No ownership or borrow checker (uses tracing GC)
- No macros or compile-time metaprogramming
- No runtime reflection
- No inheritance-based OOP
- No operator overloading
- No trait objects or dynamic dispatch
- No exceptions

See [spec §3 and §22](docs/spec/ja/language-spec.md) for the full list.

## Building from source

> Requires Rust 1.88+, LLVM 21, and Boehm GC (`bdw-gc`).

Install prerequisites:

```bash
# macOS
brew install llvm@21 bdw-gc

# Debian / Ubuntu
sudo apt install llvm-21 clang-21 libgc-dev
```

Then build:

```bash
git clone https://github.com/tyra-lang/tyra.git
cd tyra
cargo build --release -p tyra-cli
```

The binary is at `target/release/tyra`.

Tyra's reference implementation links against Boehm GC for heap
reclamation (see [ADR-0007](docs/design/0007-boehm-gc-reference-impl.md)).

## Versioning

Tyra uses two parallel version streams:

- **Specification**: tagged as `spec-v0.1.0`, `spec-v0.2.0`, ...
- **Compiler**: tagged as `v0.1.0`, `v0.1.1`, ...

The compiler always declares which spec version it implements:

```console
$ tyra --version
tyra 0.3.0
implementing language spec 0.3
```

While Tyra is at v0.x, **breaking changes are allowed in MINOR version bumps**. After v1.0, breaking changes will use the Edition model (similar to Rust editions).

## Contributing

Tyra is at a stage where the most valuable contributions are:

1. **Reading the spec** and reporting ambiguities or contradictions as issues
2. **Writing example programs** that exercise edge cases (see `bench/static-corpus/`)
3. **Translating documentation** to English

Code contributions are welcome but the architecture is still solidifying. See [CONTRIBUTING.md](CONTRIBUTING.md) and [AGENTS.md](AGENTS.md).

## Philosophy

Tyra is a bet that the next decade of software will be written in collaboration between humans and LLMs, and that this collaboration deserves a language designed for it — not an existing language retrofitted with AI tooling.

This means accepting tradeoffs:

- We choose verbosity over inference when inference creates ambiguity
- We choose one way to do things over multiple equivalent ways
- We choose explicit annotation over clever shortcuts
- We choose a small, learnable language over a powerful, expressive one

If you've ever wished a language would just *behave predictably* — that the code you read would mean what it appears to mean, that an LLM's first guess would be correct — Tyra is built for you.

## License

Apache License 2.0. See [LICENSE](LICENSE).

## Acknowledgments

Tyra's design benefited from iterative review and discussion with AI assistants during specification development. Final design decisions and project direction remain the responsibility of the maintainer.

---

**English** | [日本語](README.ja.md)
