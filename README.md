# Tyra

A statically-typed, AI-friendly programming language for backend services, CLI tools, and business applications.

> **v0.8.0** — Hindley-Milner type inference (rank-1), E0500 LLVM crash eliminated (E9001 ICE guard), `LinkedMap<K,V>` / `LinkedSet<T>` (insertion-order-preserving), E0308 heuristic iv (ADT variant suggestion), Windows native support (MSVC ABI, `vcpkg` + `lld-link`). **E0500 occurrences: 0** in AI-gen benchmark Run 18 (86/100 pass, seed=18 — seed differs from Run 17's seed=2, so pass-count comparison is not direct; see [`bench/ai-gen/results/SUMMARY.md`](bench/ai-gen/results/SUMMARY.md)). [See known limitations](#known-limitations) before using in production.

---

## What is Tyra?

Tyra is a general-purpose language designed from the ground up for an era where humans and LLMs collaborate on code. Every design decision prioritizes **interpretive consistency**: the same input should yield the same parse, the same type, the same meaning — for both humans and AI.

```tyra
import fs
import string

fn word_count(path: String) -> Result<Int, fs.FsError>
  let text = fs.read_to_string(path)?
  Ok(string.split_whitespace(text).len())
end

fn main() -> Unit
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
fn read_port() -> Result<Int, String>
  let text = fs.read_to_string("app.conf")?
  string.parse_int(text).ok_or("invalid port number")?
end

# Value types with auto-derived equality
value Point
  x: Float
  y: Float
end

let p1 = Point(x: 1.0, y: 2.0)
let p2 = p1.copy(x: 3.0)
```

## New in v0.8.0 — LinkedMap and LinkedSet

`LinkedMap<K,V>` and `LinkedSet<T>` preserve insertion order during iteration, unlike the HAMT-based `Map`/`Set` which iterate in hash order.

```tyra
import linked_map

fn main() -> Unit
  let scores: LinkedMap<String, Int> = LinkedMap.new()
  let scores = scores.insert("alice", 95)
  let scores = scores.insert("bob",   87)
  let scores = scores.insert("carol", 92)
  for name, score in scores
    print("#{name}: #{score}")   # alice, bob, carol — insertion order guaranteed
  end
  let after = scores.remove("bob")
  print("len=#{after.len()}")    # 2
end
```

```tyra
import linked_set

fn main() -> Unit
  let seen: LinkedSet<String> = LinkedSet.new()
  let seen = seen.insert("apple")
  let seen = seen.insert("banana")
  let seen = seen.insert("apple")   # duplicate — no-op
  print("len=#{seen.len()}")        # 2
  for item in seen
    print(item)                     # apple, banana — insertion order
  end
end
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

**Stable in v0.8.0** — supported and tested:

| Component | Notes |
| --- | --- |
| Language specification v0.7 | ✅ Complete |
| Lexer, Parser, Type checker | ✅ Complete |
| LLVM codegen + Boehm GC runtime | ✅ macOS arm64 / Linux x86_64 (glibc + musl) |
| Standard library: string, list, fs, io, float, json, assert, time, log | ✅ Complete |
| `tyra check / run / build` CLI (zero-arg project mode, `--release`) | ✅ Complete |
| `tyra build --static` — static single binary (musl) | ✅ Complete (v0.5.0+) |
| `tyra fmt [--check] [--stdin] <file\|dir>` — formatter + 100-col wrapping | ✅ Complete |
| `tyra test [--filter] [--list] [--format tap\|junit] [--timeout] [--jobs N]` | ✅ Complete |
| `tyra test --coverage` — line/function coverage reporting | ✅ Complete (v0.6.0+) |
| Per-test process isolation in `tyra test` | ✅ Complete (v0.5.0+) |
| Panic expectation (`test_panics_*` / `test "name" panics`) | ✅ Complete (v0.6.0+) |
| `test "name" [panics] <body> end` language syntax | ✅ Complete (v0.6.0+) |
| `continue` statement | ✅ Complete |
| `tyra new <name> [--lib] [--vcs none]` — project scaffolding | ✅ Complete |
| `tyra mod init/add/update/remove/show/tree/sync/clean [--locked]` | ✅ Complete |
| `tyra bench ai-gen` — AI generation benchmark runner | ✅ Complete |
| `tyra bench <dir>` — general-purpose wall-clock microbenchmark runner | ✅ Complete |
| Lambda / closures (spec §9.4, ADR 0011) | ✅ Complete |
| Generic `List<T>` + `map`/`filter`/`fold` | ✅ Complete |
| Generic `Map<K,V>` — HAMT-persistent, `insert`/`remove`/`get`/`contains_key`/iteration | ✅ Complete (v0.7.0+) |
| Generic `Set<T>` — HAMT-persistent, `insert`/`remove`/`contains`/iteration | ✅ Complete (v0.7.0+) |
| `for k, v in m` / `for v in s` — Map/Set iteration | ✅ Complete (v0.7.0+) |
| E0308 diagnostic improvements — help hints, secondary labels, cascade dedup | ✅ Complete (v0.7.0+) |
| E0313 — for-loop binding count mismatch diagnostic | ✅ Complete (v0.7.0+) |
| Generic `assert.eq` / `assert.ne` (Int, String, Bool) | ✅ Complete |
| `string.replace` / `string.join` | ✅ Complete (v0.5.0+) |
| `Tyra.lock` + floating `branch` constraints + transitive dep resolution | ✅ Complete |
| LSP server (`tyra-lsp`) + VS Code extension | ✅ Development install |
| DAP debugger (DWARF + lldb-dap + VS Code breakpoints/locals) | ✅ Complete (v0.6.0+) |
| Static conformance corpus (33 positive programs + 21 error cases) | ✅ CI-gated |

## Platform support

> **Canonical reference.** This section is the single source of truth for platform and link-mode support. All other docs (AGENTS.md, release notes) refer here.

| Platform | Binary type | Status |
|----------|-------------|--------|
| Linux x86_64 (glibc) | Dynamic | Supported |
| Linux x86_64 (musl) | Static | Supported (v0.5.0+) |
| macOS arm64 | Dynamic | Supported |
| Windows x86_64 (MSVC) | Dynamic (`gc.dll` same-dir) | Supported (v0.8.0+, MSVC ABI, `gc.dll` same-dir) |

**Using the musl static release artifact:**

The `tyra-*-linux-musl-x86_64-static.tar.gz` release includes a pre-built static `examples/hello` binary. To verify static linking works on your system:

```bash
tar xzf tyra-*-linux-musl-x86_64-static.tar.gz
cd tyra-*/
./examples/hello        # prints: hello, tyra
file examples/hello     # should say: statically linked
```

To compile your own static binary, use the musl-targeting `tyra` (i.e. run on Alpine Linux or equivalent musl toolchain):

```bash
tyra build --static myprogram.tyra
```

**Experimental in v0.4.0** — included but not production-ready:

| Component | Notes |
| --- | --- |
| `http.server` stdlib | ⚠️ Basic GET/POST routing only; not production-ready |

**Backlog** — not yet implemented:

| Component | Notes |
| --- | --- |
| Registry (`tyra publish`), full registry-backed resolver | ⏳ Future |
| Pre-built binaries (homebrew, apt) | ⏳ Later |
| VS Code Marketplace publication | ⏳ Later |

## Known limitations

- **Windows**: supported on x86_64-pc-windows-msvc (v0.8.0+, MSVC ABI). `tyra build` auto-copies `gc.dll` next to the output binary; no PATH change needed. MinGW GNU ABI is not supported. Windows ARM64 and native PDB debug symbols are v0.9+.
- **`LinkedMap.remove` / `LinkedSet.remove` is O(n)**: the entries array is rebuilt on each remove. For workloads with frequent removals, use `Map` / `Set` instead.
- **HM unification is conservative**: `types_compatible()` uses a per-call throw-away substitution rather than propagating the substitution across the full checker. Full substitution threading is deferred to v0.9. Most programs are unaffected; edge cases may surface unexpected type errors.
- **`tyra build --static`**: only reliable on musl. glibc static linking is unsupported (breaks `getaddrinfo`).
- **`http.server`**: experimental. Single-threaded, no TLS, no middleware. Do not use in production.
- **Breaking changes**: expect breaking changes before v1.0.

## Documentation

- **[Getting Started](docs/getting-started/README.md)** — installation, hello world, testing, and project lifecycle
  - [Project Lifecycle](docs/getting-started/09-project-lifecycle.md) — `tyra new`, `tyra mod`, dependencies, builds
  - [Debugging](docs/getting-started/10-debugging.md) — DAP debugger, VS Code breakpoints, lldb-dap setup
- **[Language Specification (Japanese)](docs/spec/ja/language-spec.md)** — the authoritative source of truth
- **[Language Specification (English)](docs/spec/en/language-spec.md)** — translation, may lag behind
- **[Design Decisions](docs/design/)** — architecture decision records explaining *why*
- **[RFCs](docs/rfcs/)** — proposed changes for future versions
- **[Examples](examples/)** — runnable programs demonstrating stdlib features
  - [examples/11-stdlib-time-log.tyra](examples/11-stdlib-time-log.tyra) — `time.now_unix`, `time.monotonic_millis`, `log.info/warn/error`

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
tyra 0.8.0
implementing language spec 0.8
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
