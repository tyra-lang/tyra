# Tyra

A statically-typed, AI-friendly programming language for backend services, CLI tools, and business applications.

> ⚠️ **Pre-alpha**: Tyra is under active development. The language specification is at v0.1 (Draft). Breaking changes are expected before v1.0.

---

## What is Tyra?

Tyra is a general-purpose language designed from the ground up for an era where humans and LLMs collaborate on code. Every design decision prioritizes **interpretive consistency**: the same input should yield the same parse, the same type, the same meaning — for both humans and AI.

```tyra
import http.server

export async fn main() -> Result<Unit, AppError>
  let app = server.new()
  app.get("/health", health_handler)
  app.listen(port: 8080).await?
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
- **Has one official toolchain**: build, test, fmt, deploy in a single CLI

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

## Status

| Component | Status |
| --- | --- |
| Language specification v0.1 | ✅ Draft complete |
| Lexer | 🚧 In progress |
| Parser | ⏳ Planned |
| Type checker | ⏳ Planned |
| LLVM codegen | ⏳ Planned |
| Standard library | ⏳ Planned |
| Tooling (fmt, lsp, mod) | ⏳ Planned |

There is **no working compiler yet**. If you want to read the design, see the spec. If you want to use Tyra, please wait for v0.1.0 release.

## Documentation

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

## Non-goals (v0.1)

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

> Requires Rust 1.85+ and LLVM 21.

```bash
git clone https://github.com/tyra-lang/tyra.git
cd tyra/compiler
cargo build --release
```

The compiler is not yet functional. This builds the skeleton.

## Versioning

Tyra uses two parallel version streams:

- **Specification**: tagged as `spec-v0.1.0`, `spec-v0.2.0`, ...
- **Compiler**: tagged as `v0.1.0`, `v0.1.1`, ...

The compiler always declares which spec version it implements:

```console
$ tyra --version
tyra 0.1.0
implementing language spec 0.1
```

While Tyra is at v0.x, **breaking changes are allowed in MINOR version bumps**. After v1.0, breaking changes will use the Edition model (similar to Rust editions).

## Contributing

Tyra is at a stage where the most valuable contributions are:

1. **Reading the spec** and reporting ambiguities or contradictions as issues
2. **Writing example programs** that exercise edge cases (see `tests/corpus/`)
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

Tyra's design was developed through extensive collaboration with Claude (Anthropic). The language is, in a sense, the first to have its specification iteratively reviewed by an AI — a meta-validation of its core thesis.

---

**English** | [日本語](README.ja.md)
