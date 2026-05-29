# Tyra Project Strategy

- **Version**: 1.4
- **Status**: Active
- **Last updated**: 2026-05-27
- **Owner**: Project lead
- **Review cadence**: Every 3 months, or after any major spec change

This document captures the strategic positioning, competitive landscape, target users, and decision framework for the Tyra language project. It is the **single source of truth for "why Tyra exists"** and should be consulted before any major direction change.

For technical specifications, see `docs/spec/ja/language-spec.md`.
For design decisions, see `docs/design/`.
For AI assistant guidance, see `AGENTS.md` (which references this document).

---

## 1. Executive Summary

Tyra is a Ruby-readable, statically-typed, natively-compiled programming language designed for AI-assisted software development. It targets backend services, CLI tools, and business applications.

**One-sentence positioning**:

> Tyra is a Ruby-readable native language that strips Crystal's metaprogramming, mirrors Go's operational simplicity, and constrains itself more strictly than Gleam or V — designed to be auditable by both humans and AI.

**Strategic thesis**:

The convergence of three trends creates an opening for Tyra:

1. **AI-assisted coding** is now mainstream, and existing languages were not designed for it
2. **Crystal** has stagnated in the "Ruby + static types + native" niche, with experimental parallelism and exception-based errors
3. **Operational simplicity** (Go-style toolchain, single binary) is increasingly expected as a baseline

Tyra's bet is that **a language designed from scratch for AI auditability and strict semantics, with operational standards matching Go and surface readability matching Ruby, can capture users dissatisfied with Crystal, V, and parts of the Go and Ruby ecosystems.**

**Current status**: v0.8.0 released (2026-05-29). v0.1.0 shipped the compiler, runtime, and Tier 1 stdlib for macOS arm64 and Linux x86_64. v0.2.0 added `tyra fmt`, `tyra test`, `stdlib/assert`, the `continue` statement, and runtime fixes. v0.3.0 delivered the full project lifecycle (`Tyra.toml`, `tyra new`, `tyra mod`, three-layer import resolution, zero-arg commands). v0.4.0 adds lambda / closures (ADR 0011), generic `List<T>` with `map`/`filter`/`fold`, generic `assert.eq<T>`, `tyra bench <dir>`, `tyra test --timeout/--jobs`, and `Tyra.lock` with floating `branch` constraints and transitive dependency resolution. v0.5.0 adds a cross-OS CI gate (Linux glibc + macOS arm64 + Alpine musl required on every PR), `tyra build --static` producing a static single binary on musl (addressing strategy §4.2 vs Crystal's Alpine-only static linking), `string.replace`/`string.join` stdlib functions, and per-test process isolation in the test runner. v0.6.0 (codename Harmonic Moore) ships generic `Map<K,V>` and `Set<T>`, `time`/`log` stdlib modules, `test "name"` syntax with `panics` modifier, runner-native panic expectation, `tyra test --coverage` (line/function), and the DAP debugger (DWARF + lldb-dap + VS Code integration). Phase 1 deliverable #8 (debugger) is now shipped. v0.7.0 (codename Polymorphic Star) ships E0308 diagnostic improvements (help hints, secondary labels, cascade dedup), HAMT-based persistent `Map<K,V>` and `Set<T>` with `insert`/`remove`/iteration, E0313 for-loop binding mismatch diagnostic, and `inkwell 0.9` dependency with LLVM version auto-detection in CI (full IR migration deferred to v0.8+). Post-release diagnostic hardening: E0110/E0211/E0213 help hints, E0213 new error code (replaces BUG panic for fn-main + top-level coexistence), E0204 promoted to hard compile error via `Program::lower_errors`, `List<T>`/`Option<T>` instance method dispatch in type checker (eliminates E0500 LLVM crashes from Ty::Error cascade). AI-gen benchmark Run 16 (v0.7.0, E0204 hard-error 前): 91/100 pass (91%). Run 17 (final, post-hardening): 98/100 pass (98%). Residual 2%: 1× E0500 codegen edge case, 1× AI syntax error. v0.8.0 (codename Lexical Bengio) ships rank-1 Hindley-Milner type inference (TyVarId + Substitution + occurs-check unify()), E9001 ICE diagnostic (Ty::Error/Ty::Var reaching codegen fails cleanly), E0308 heuristic (iv) ADT variant suggestion, `LinkedMap<K,V>` and `LinkedSet<T>` insertion-order persistent collections (ADR-0019), Windows native support via MSVC ABI (ADR-0021, `lld-link.exe` + `gc.dll` same-dir, `release-gate-windows` now required), and `strtol`→`strtoll` LLP64 fix.

---

## 2. Target Users and Acquisition Strategy

### 2.1 Primary acquisition targets

Ranked by feasibility and strategic priority:

| Rank | Source | User profile | Estimated TAM | Acquisition difficulty |
| -- | -- | -- | -- | -- |
| 1 | Crystal users dissatisfied with macros, exceptions, or experimental parallelism | Backend developers who tried Crystal but adopted alternatives | Hundreds to low thousands | Medium |
| 2 | Go users hitting type-system limits (no ADTs, weak generics, nil pointer issues) | Backend/CLI developers tired of `if err != nil` boilerplate | Tens of thousands (subset of millions of Go users) | High |
| 3 | V users concerned about `unsafe`, `autofree` (WIP), or missing debugger | Developers attracted to V's simplicity but blocked by maturity | Hundreds | Medium |
| 4 | Ruby developers wanting compile-time safety without losing readability | Rails developers exploring TypeScript, Crystal, or Sorbet | Thousands (subset of hundreds of thousands) | High |
| 5 | Gleam users dissatisfied with BEAM constraints | Developers who like Gleam's design but need native deployment | Hundreds | Low — Gleam users tend to stay in BEAM ecosystem |

**Key insight**: The largest TAM (Go users) is also the hardest acquisition. The most realistic initial wins come from **Crystal and V users** — smaller pools, but with concrete dissatisfaction Tyra can address.

### 2.2 Acquisition narratives

Each target requires a different narrative:

**For Crystal users**:

> "What Crystal would look like if designed after Rust and Go proved that explicit error handling is better than exceptions, and after the LLM era proved that constrained syntax is better than expressive freedom. No macros, no `responds_to?`, no exception-based control flow. Single binary on every OS, not just Alpine."

**For Go users**:

> "Go's operational simplicity, plus real algebraic data types, exhaustive pattern matching, Result/Option for explicit errors, and Ruby-readable syntax. The build/test/fmt/deploy story is the same; the type system is dramatically better."

**For V users**:

> "V's static typing and native binaries, with stricter semantics, no `unsafe`, no `autofree`-style WIP features, and a debugger. Less expressive freedom in exchange for predictability and AI auditability."

**For Ruby users**:

> "Ruby's surface readability with compile-time null safety and visible error paths. Not a Ruby successor — a Ruby-influenced language with different philosophy."

**For Gleam users** (low priority):

> "Gleam's safety without BEAM's runtime. Imperative style instead of functional. Native single binary instead of escript or Docker."

### 2.3 Anti-targets (users we will NOT pursue)

- **Rust users**: Tyra's GC and weaker safety model do not appeal to users who chose Rust for ownership and zero-cost abstractions
- **Python users**: Ecosystem gap is decisive; Python users don't choose new languages over ecosystem
- **TypeScript users**: Web/JS ecosystem lock-in is too strong
- **High-availability soft-realtime systems builders**: Gleam/Erlang/Elixir are decisively better
- **Systems programmers**: Use Rust, Zig, or C
- **Frontend developers**: Use TypeScript, Elm, or Rust+WASM
- **Game developers**: Out of scope (no real-time, no specific tooling)

---

## 3. Competitive Landscape

### 3.1 The 5-layer competitive model

Tyra's competitors are not a flat list. They occupy distinct positions:

| Layer | Language | Relationship | What they own |
| -- | -- | -- | -- |
| **Direct design competitor** | Crystal | Closest in surface form | "Ruby + static types + LLVM-native" |
| **Strategic benchmark** | Go | Operational gold standard | "Single binary + integrated toolchain + production maturity" |
| **Philosophical competitor** | Gleam | Shared design values | "Type-safe + Result-based + no exceptions + AI-friendly determinism" |
| **Message-space competitor** | V | Overlapping marketing | "Simple + fast + safe + compiled + no null + Option/Result" |
| **Syntactic ancestor** | Ruby | Source of surface syntax | "Developer experience + readability + meta-flexibility" |

Each requires a different competitive response (see §3.2).

### 3.2 Differentiation by competitor

#### vs Crystal (Layer 1: Direct competitor)

**What Crystal does well**: Mature stdlib, Ruby compatibility, compile-time macros, established community of thousands.

**What Crystal does poorly**:

- Parallelism is officially "experimental" since the language began (over 10 years)
- Static binary linking is "Alpine Linux only" per official docs
- Errors use exceptions, with no Result type in stdlib
- `struct` allows mutable fields (`property`)
- Float can be compared with `==`, NaN bugs slip through
- No semantic ability auto-derivation (Crystal `Struct` auto-generates `==` but has no rules like "mut fields block Hash")

**Tyra wins by**:

- Result/Option with `?` propagation as the standard error model
- True parallelism from day one (not experimental)
- Single static binary on every major OS
- Strict `value`/`data` distinction with enforced immutability
- Float has no `Eq` (ADR-0002 prevents NaN bugs)
- Ability auto-derivation with semantic rules
- No macros, no operator overloading, no runtime reflection (AI auditability)

**Crystal wins on**: Years of ecosystem, mature stdlib, established community, easier Ruby migration path.

**Strategic note**: Crystal is the closest existing language to Tyra. If Tyra cannot demonstrate clear advantages over Crystal, the project has no reason to exist.

#### vs Go (Layer 2: Strategic benchmark)

**What Go does well**: Massive ecosystem (millions of users, tens of thousands of production deployments), 14+ years of stability, decisive operational simplicity, generic implementation as of Go 1.18.

**What Go does poorly** (from a type-system perspective):

- No algebraic data types (sum types must be simulated with interfaces)
- No exhaustive pattern matching
- Nil pointer dereferences remain a major bug source
- Error handling boilerplate (`if err != nil { return err }`) is widely criticized
- All types are mutable by default

**Tyra wins by** (in theory):

- Real ADTs with exhaustive `match`
- `Result`/`Option` + `?` eliminates `if err != nil` chains
- `Option` eliminates nil pointer bugs at compile time
- `value`/`data` distinction makes mutability intentional
- Ruby-readable surface (Go's syntax is occasionally awkward)

**Go wins on**: Everything except the type system. Ecosystem, stability, hiring market, deployment know-how, library availability.

**Strategic note**: Tyra does NOT aim to displace Go. Go is the **operational benchmark** ("Can Tyra build/test/deploy as simply as Go?"). Some Go users dissatisfied with the type system may migrate, but Tyra's success does not depend on capturing Go's market.

#### vs Gleam (Layer 3: Philosophical competitor)

**What Gleam does well**: First-class type safety, Result-based error handling, no null, BEAM's industry-leading concurrency and fault tolerance, JavaScript target, integrated toolchain, official v1.0 stable since March 2024.

**What Gleam does that Tyra cannot match**:

- BEAM's per-process garbage collection and preemptive scheduling
- OTP's supervision trees for fault tolerance
- Ability to interop with Erlang/Elixir ecosystem
- Hot code reload
- Distributed systems primitives

**Tyra wins by**:

- Native single-binary deployment (Gleam needs `escript` or BEAM runtime)
- Imperative style (Gleam is functional)
- Surface syntax familiar to Ruby/Swift/Go developers
- Faster cold start (no VM warmup)
- Lower memory baseline for small services

**Strategic note**: Gleam owns the high-availability concurrent server domain. Tyra should NOT compete there. The competition is for "modern type-safe language" mindshare in non-BEAM domains.

#### vs V (Layer 4: Message-space competitor)

**What V does well**: Self-promoted as "weekend-learnable," fast compilation, small binaries, immutable by default, Option/Result, no null, native binary, C interop, 7+ years of development, 37k+ GitHub stars.

**What V does poorly**:

- `autofree` is officially "still WIP… avoid using it"
- Native backend has "no debug support" per official docs
- Parser-less C interop requires manual declaration
- Pre-1.0 status with feature freezes still pending
- Smaller ecosystem (~621 packages on VPM)

**Tyra wins by**:

- Production-ready debugger from launch (planned)
- Mature, stable runtime (no WIP features in release)
- Stricter semantics (no `unsafe` escape hatches)
- Convention fixity (formatter-enforced, no syntactic alternatives)
- Argument labels for self-documenting APIs
- Stricter `value`/`data` enforcement

**Strategic note**: V owns "simple/fast/safe" in the marketing space. Tyra cannot win on these slogans. Tyra must compete on **predictability and team-deployable convention fixity**.

#### vs Ruby (Layer 5: Syntactic ancestor)

**What Ruby does well**: Massive Rails ecosystem, decades of refinement, world-class developer experience, metaprogramming flexibility, RubyGems (192,000+ packages), strong community.

**What Ruby does poorly**:

- Dynamic typing leads to runtime errors that static type systems prevent
- `nil` is the default for missing values, causing `NoMethodError`
- Performance is significantly slower than compiled languages
- Deployment requires runtime + dependency management
- Concurrency limited by GIL in MRI (Ractor exists but is constrained)

**Tyra wins by**:

- Compile-time type safety
- Native execution (significantly faster)
- Single binary deployment (no runtime needed)
- Eliminates entire bug classes (nil, type confusion)

**Tyra cannot compete on**: Ecosystem size, community, Rails-equivalent frameworks, hiring market, decades of operational know-how.

**Strategic note**: Ruby is NOT a competitor in the same way Crystal is. Ruby is **the source of expectations Tyra must manage**. Ruby developers approaching Tyra expecting "compiled Ruby" will be disappointed by missing dynamic features. Documentation must clarify "Ruby-readable but stricter" prominently.

### 3.3 Battles Tyra should avoid

Engaging in any of these is a strategic error:

- **"Simpler than V"** — V owns simplicity. Tyra is more constrained, not simpler.
- **"Faster than Crystal"** — Crystal optimizes well. Tyra has no inherent speed advantage.
- **"Better than Gleam at type safety"** — Gleam is excellent. Differentiate on style and platform, not safety.
- **"The new Ruby"** — Crystal already tried. Ruby users mostly stayed with Ruby.
- **"Replacement for Go"** — Go is too entrenched. Go is a benchmark, not a market.

---

## 4. The Three Axes of Victory

If Tyra succeeds, it will be on these three axes simultaneously. Failing on any one likely means project failure.

### 4.1 AI auditability (most differentiating, hardest to prove)

Claim: Tyra's design (no macros, no overloading, no implicit conversions, fixed formatter, single-interpretation syntax) makes AI-generated code measurably more reliable.

Required evidence:

- Same prompt generates compilable Tyra code more reliably than Crystal/V/Ruby code
- AI refactors of Tyra code break less often than equivalent refactors in other languages
- AI code review on Tyra produces fewer false positives

How to validate: Build a benchmark suite of 100 prompts, generate code with Claude/GPT in 5 languages, measure compilation success and post-edit stability.

If validated, this becomes Tyra's strongest unique selling point.

### 4.2 Crystal's structural weaknesses (immediate, concrete)

Crystal has two officially-documented weaknesses that Tyra can directly address:

1. **Parallelism is experimental** (10+ years)
2. **Static binary linking is Alpine-only**

Required evidence:

- Tyra compiles to a static single binary on Linux (glibc and musl), macOS (x86 and ARM), and Windows (long-term target; v0.1 ships Linux x86_64 + macOS arm64 only — see §6.2)
- Tyra's parallel execution is stable and benchmarked (Phase 1 milestone; v0.1.0 does not yet expose stable parallel runtime — tracked as a Phase 1 deliverable in §6.2)

How to validate: CI matrix builds across platforms, parallel benchmark suite published.

If achieved, this becomes Tyra's most concrete and hard-to-dismiss differentiator.

### 4.3 Go-level operational simplicity (table stakes)

Tyra must match Go's operational standards as a baseline. Failing to match Go means failing in the market.

Required:

- `tyra new`, `tyra run`, `tyra build`, `tyra test`, `tyra fmt`, `tyra mod` all work (Phase 1 target; v0.1.0 ships `tyra check` / `run` / `build` only — `test` / `fmt` / `mod` / `new` tracked in §6.2)
- Single command produces a release binary
- Cross-compilation supported
- LSP, debugger, formatter all official
- Documentation generation built in
- Zero-config defaults that work

How to validate: A new user can install Tyra and ship a CLI tool to GitHub releases in under 30 minutes.

This is not differentiation — it is **table stakes**. Falling short here disqualifies Tyra regardless of other strengths.

---

## 5. The Five-Language Comparison

### 5.1 Investor-facing summary

| Language | Their strength | Tyra's differentiation | Threat level | Verdict |
| -- | -- | -- | -- | -- |
| **Crystal** | Ruby-like syntax + static types + native compilation + compile-time macros, ~10 years of ecosystem | Strips Crystal's metaprogramming; adds Result/Option/?; enforces strict immutability; AI-auditable convention fixity; fixes Crystal's experimental parallelism and Alpine-only static linking | **Maximum** | Direct design competitor. Tyra's existence depends on clearly beating Crystal. |
| **Go** | Massive ecosystem, decisive operational simplicity, 14+ years of stability, millions of users | Adds real ADTs, exhaustive match, Result/Option/?, Ruby-readable syntax, immutable-by-default; matches Go's operational standards | **Maximum** | Strategic benchmark. Borrow Go's standards, do not attempt to displace Go's market. |
| **Gleam** | Type-safe, BEAM's fault tolerance and concurrency, JavaScript target, established v1.0 since March 2024 | Native single binary, imperative style, faster cold start, lower memory baseline; Ruby/Swift/Go-influenced surface syntax | **Medium-High** | Philosophical competitor. Different platforms, overlapping mindshare. |
| **V** | Self-promoted as "weekend-learnable," fast compilation, small binaries, immutable by default, Option/Result, native binary | Stricter semantics, no `unsafe`, no WIP features, official debugger, formatter-enforced convention fixity, argument labels | **Medium-High** | Message-space competitor. Cannot win on "simpler/faster"; must win on predictability. |
| **Ruby** | Rails ecosystem, decades of refinement, world-class DX, metaprogramming flexibility, 192k+ gems | Compile-time type safety, native execution, single binary deployment, eliminates nil/type bugs | **Medium** | Migration source rather than direct competitor. Manage expectations carefully. |

### 5.2 Developer-facing summary

| Language | Where they overlap | Tyra's winning angle | Risks |
| -- | -- | -- | -- |
| **Crystal** | Ruby-like syntax, static types with inference, native, type recovery via union types | No macro dependency, smaller semantics, AI-resilient code style, Go-style integrated CLI; `value`/`data` separation, trait/ability separation, fixed formatter | "Ruby + compiled + typed" alone looks like Crystal copying; must show concrete macro-free patterns |
| **Go** | Single CLI, standard formatter, deployment ease, single binary | Richer types: `Option`, `Result`, ADTs, `match`, no null/truthy | Go's strength is mature operations and ecosystem; not winnable short term |
| **Gleam** | Small consistent language, Result over exceptions, safety focus | Imperative readability without functional ceremony; direct fit for Web backend / CLI / business apps | Just being "AI-friendly + explicit + safe" looks like Gleam's philosophy retold |
| **V** | No null, Option/Result, immutable by default, native binary | Compete not on "fast/small" but on "no ambiguity / no surprise"; win via narrow specs | Speaking the same simple/fast/safe language as V makes the difference invisible |
| **Ruby** | `end` blocks, readability focus, friendly aesthetics | Required parens, public API types required, dynamic features suppressed, syntax stable for AI generation | Ruby users will scrutinize "why Tyra over Ruby"; without Rails-tier assets, frontal attack fails |

### 5.3 When to NOT use Tyra

A practical guide for developers and consultants:

| Scenario | Use instead | Why |
| -- | -- | -- |
| WebSocket server with thousands of connections | Gleam, Elixir | BEAM's concurrency model is decisive |
| Embedded systems, kernel work | Rust, Zig, C | Tyra has GC and no low-level control |
| Quick scripts, REPL exploration | Python, Ruby | Tyra requires compilation, no REPL |
| Browser frontend | TypeScript, Elm, Rust+WASM | Tyra's WASM story is not yet first-class |
| AI/ML model training | Python | Ecosystem is decisive |
| Existing Ruby/Rails codebase | Stay with Ruby | Migration cost is enormous |
| Existing Go production system | Stay with Go | No compelling reason to migrate |
| You need 10,000+ ready-to-use libraries today | Anything but Tyra | Tyra's ecosystem is zero |

Tyra is for **new projects** in **backend services, CLI tools, and business applications** where the team values type safety and AI-assisted development.

---

## 6. Strategic Roadmap

### 6.1 Phase 0: Specification (COMPLETE)

- ✅ Spec v0.1 Draft (1267 lines)
- ✅ 6 ADRs (data semantics, Float Eq, stdlib scope, ? unification, multi-constraint generics, top-level expressions)
- ✅ Phase 0a (10 example programs in Tyra)
- ✅ Phase 0b (comparison with Gleam, V, Ruby, Crystal)
- ✅ Strategic positioning document (this file)

### 6.2 Phase 1: LLVM implementation (12-24 months)

**v0.1 platform scope**: macOS arm64 and Linux x86_64. Windows is out of scope for v0.1 (see [ADR-0007](design/0007-boehm-gc-reference-impl.md)); item 10 below targets a future release.

**Goal**: A production-grade compiler that delivers on Tyra's promises.

Deliverables (in order):

1. Lexer + parser + AST + type checker (in Rust)
2. AI generation benchmark: 100 prompts, compare Tyra/Crystal/V/Gleam/Ruby compilation success rates (run early, as soon as the parser can validate syntax)
3. MIR (mid-level IR) with desugaring
4. LLVM IR generation
5. Runtime (GC + async scheduler) in Rust/C
6. Standard library Tier 1 implementation
7. Standard library Tier 2 (http, fs, json, etc.) — core APIs (fs, json, http, string, list, map) frozen and shipped in v0.1.0; broader Tier 2 (collections, time, test, log, float) continues as Phase 1 work
8. LSP, formatter, debugger
9. Documentation site, comparison pages
10. CI matrix builds for Linux/macOS/Windows

Note: the prototype transpiler phase was removed from the roadmap. Phase 0's thorough spec-by-example validation (10 programs, 6 ADRs, 5-language competitive analysis) mitigates the risk of discovering fundamental design flaws during expensive compiler work.

**Success criteria**:

- AI generation benchmark shows Tyra has measurably higher compilation success than at least 2 competitors
- Single binary builds on all major OSes (beating Crystal's Alpine-only)
- Stable parallel execution (beating Crystal's "experimental")
- LSP and debugger work in VS Code from day one
- Tier 1 stdlib stable
- 50+ third-party programs publicly available

### 6.3 Phase 2: Adoption (ongoing after Phase 1)

**Goal**: Sustainable community of contributors and users.

Deliverables:

- Public website with comparison pages (vs Crystal, vs Go, vs Gleam, vs V, vs Ruby)
- Migration guides from Crystal and V
- 10+ production case studies
- Conference talks at relevant events (RubyKaigi, Strange Loop, etc.)
- Sponsored hosting for documentation, package registry

**Success metrics**:

- 1,000+ GitHub stars within 6 months of Phase 1 completion
- 10+ companies with production Tyra code
- Active contributor base (10+ regular committers)

---

## 7. Risk Analysis

### 7.1 Existential risks

These could end the project. Address them proactively.

#### Risk: AI-friendliness is not measurable or not differentiating

- Probability: Medium
- Impact: Total
- Mitigation: Design the AI benchmark early in Phase 1. If results are weak, pivot to "Crystal's structural weaknesses" as primary differentiator.

#### Risk: Phase 1 takes 5+ years and team loses momentum

- Probability: Medium-High
- Impact: Total
- Mitigation: Strict scope discipline. Tier 1 stdlib only. Defer Tier 2 to community. Reuse existing tooling (LLVM, rustc patterns) where possible.

#### Risk: Crystal team adds Result type and fixes parallelism

- Probability: Low (10+ year inertia)
- Impact: High (Tyra's structural advantages disappear)
- Mitigation: Speed of execution. Ship Tyra v1.0 before Crystal can react. Even if Crystal catches up later, Tyra would have established its niche.

#### Risk: A larger competitor (e.g., Apple, Google) launches a similar language

- Probability: Low
- Impact: Total
- Mitigation: None feasible. Focus on speed and unique positioning (Ruby-readable + AI-auditable). A corporate-backed language would likely have different priorities.

### 7.2 Strategic risks

These would limit Tyra's success without ending it.

#### Risk: Cannot match Go's operational standards

- Probability: Medium
- Impact: High (table stakes failure)
- Mitigation: Treat operational tooling as P0, not P2. Allocate equal effort to compiler and toolchain.

#### Risk: Documentation fails to clarify "Ruby-readable but stricter"

- Probability: High
- Impact: Medium (Ruby users tries Tyra, gets disappointed, writes negative review)
- Mitigation: Front-load expectation management in README, comparison pages, and onboarding tutorials.

#### Risk: Marketing message overlaps with V indistinguishably

- Probability: Medium
- Impact: Medium (Tyra blends in, no acquisition advantage)
- Mitigation: Never use V's slogans ("simple, fast, safe, compiled"). Always emphasize convention fixity and AI-auditability.

#### Risk: Implementation diverges from spec

- Probability: Medium
- Impact: High (community fragmentation)
- Mitigation: Conformance test suite from day one. AGENTS.md enforces spec authority. Any divergence requires ADR.

### 7.3 Organizational risks

#### Risk: Solo maintainer burnout

- Probability: High
- Impact: Total
- Mitigation: Document everything (this strategy doc serves that purpose). Build for community contribution from the start.

#### Risk: Spec churn after v1.0

- Probability: Medium
- Impact: High (early adopter resentment)
- Mitigation: Strict ADR process. Edition model (Rust-style) for v2.0+ breaking changes.

---

## 8. Success Definitions

### 8.1 Three possible success modes

Tyra can succeed in three different ways. Each has different implications.

#### Mode A: Niche language with loyal users (most likely)

- 1,000-10,000 active users
- 50-500 production deployments
- Active Discord/forum
- Self-sustaining via community contributions
- Comparable to Gleam's current position

This is the most realistic success mode. It validates the design without requiring market disruption.

#### Mode B: Crystal's successor (ambitious)

- Captures 30-50% of Crystal users (~1,000-3,000)
- Becomes the default "Ruby + types + native" choice
- Crystal community acknowledges Tyra as the modern alternative
- Documentation, tooling, and ecosystem rival Crystal

This requires Phase 1 to be exceptionally well-executed and visible.

#### Mode C: Mainstream backend language (very unlikely)

- 100,000+ users
- Adoption at major tech companies
- Competes with Go for new project starts
- Featured in major industry surveys

This requires either a viral moment, a major corporate sponsor, or a decade of compounding growth. Plan for Mode A; allow for Mode B; do not bet on Mode C.

### 8.2 What "failure" looks like

The project should be considered a failure (and either pivoted or shut down) if:

- Phase 1 compiler fails to attract any third-party users
- AI benchmark shows no measurable advantage over competitors
- Phase 1 stretches beyond 36 months without a usable compiler
- Spec requires fundamental redesign discovered during implementation
- Solo maintainer becomes unable to continue and no community has formed

### 8.3 Honorable outcomes even on failure

Even if Tyra fails to gain adoption, the project produces lasting value:

- A complete language specification with 6 ADRs (educational artifact)
- Insights about AI-assisted language design (potential blog series, talks, paper)
- Reference implementation of "Ruby-readable + statically-typed + native" patterns
- Validation of which design choices work for AI auditability (negative results have value)

These outcomes alone justify the investment if the strategic outcomes do not materialize.

---

## 9. Decision Framework

When facing a non-obvious decision about Tyra, apply this framework:

### Step 1: Does the spec already answer this?

If yes, follow the spec. Do not improvise.

### Step 2: Does an ADR address this?

If yes, follow the ADR's logic. If you disagree, propose a new ADR superseding it.

### Step 3: Does this align with the 5-layer competitive position?

Ask:

- Does Crystal have it? If yes, why is Tyra excluding/changing it?
- Would Go reject it as too clever? If yes, Tyra should also reject.
- Does Gleam have a competing approach? If yes, why is Tyra's better?
- Does V claim it as a selling point? If yes, Tyra needs a different story.
- Would Ruby users misinterpret it? If yes, document the difference.

### Step 4: Does this advance one of the three axes of victory?

- AI auditability
- Crystal's structural weaknesses
- Go-level operational simplicity

If yes, prioritize. If no, deprioritize.

### Step 5: When in doubt

Prefer **less power, more determinism**. Tyra wins by being more predictable, not by being more powerful.

---

## 10. Strategy Map (Visual Summary)

```text
[Tyra's Position]

    Type Safety
         ↑
         │  Rust ●
         │
   Gleam ●     Tyra (target position)
         │  ●          
         │     ● Crystal
         │  ● V
         │
         │            ● Go
         │
         │      ● Ruby
         │              ● Python
         └────────────────────→ Operational Simplicity


[Acquisition Funnel — Realistic Estimate]

  Crystal users dissatisfied ──→ Tier 1 target (★★★★)
  V users wanting maturity   ──→ Tier 1 target (★★★)
  Ruby users wanting types   ──→ Tier 2 target (★★)
  Go users hitting type      ──→ Tier 2 target (★★)
  Gleam users                ──→ Tier 3 target (★)
  Rust/Python/TS users       ──→ NOT targeted


[Three Axes of Victory]

  AI Auditability ────────┐
                          ├───→ Tyra Success
  Crystal's Weaknesses ───┤
                          │
  Go-level Operations ────┘


[Battles to Avoid]

  ✗ "Simpler than V"
  ✗ "Faster than Crystal"  
  ✗ "Better than Gleam"
  ✗ "Replacement for Go"
  ✗ "The new Ruby"
```

---

## 11. Maintenance of This Document

This strategy document should be:

- **Reviewed every 3 months** by the project lead
- **Updated after any major spec change** (new ADR, breaking change)
- **Updated after Phase 1 results** (validate or invalidate the AI auditability hypothesis)
- **Updated if a new competitor emerges** (e.g., Apple/Google launches a similar language)
- **Updated if acquisition data invalidates the target user model**

When updating:

1. Increment the version number at the top
2. Add a changelog entry below
3. Notify any active contributors

### Changelog

- **v1.0 (2026-04-15)**: Initial document. Captures Phase 0a/0b conclusions, 5-layer competitive model, 5-language comparison, 3 axes of victory, risk analysis.
- **v1.1 (2026-04-15)**: Remove prototype transpiler phase (old Phase 1). Renumber: LLVM implementation is now Phase 1, adoption is Phase 2. Go directly from spec to compiler.
- **v1.2 (2026-05-23)**: Updated §1 current status for v0.5.0 release (cross-OS CI matrix, musl static binary, string.replace/join, per-test isolation).
- **v1.3 (2026-05-25)**: Updated §1 current status for v0.6.0 release (generic Map/Set, time/log stdlib, test "name" syntax, panic expectation, coverage, DAP debugger).
- **v1.4 (2026-05-27)**: Updated §1 current status for v0.7.0 release (E0308 diagnostics, HAMT persistent Map/Set, Map/Set iteration, inkwell dependency).
- **v1.5 (2026-05-28)**: Updated §1 for v0.7.0 post-release hardening (E0204 hard error, E0213, E0110/E0211 help hints, List/Option method dispatch, AI-gen Run 16 results).
- **v1.6 (2026-05-28)**: Updated §1 for AI-gen Run 17 final benchmark (98/100 pass, 98.0%; hardening 後の最終測定値).
- **v1.7 (2026-05-29)**: Updated §1 current status for v0.8.0 release (HM type inference, E9001, E0308 heuristic iv, LinkedMap/LinkedSet, Windows MSVC ABI, strtol→strtoll).

---

## 12. References

### Internal documents

- `docs/spec/ja/language-spec.md` — Language specification (authoritative)
- `docs/spec/en/language-spec.md` — English translation
- `docs/design/0001-adt-data-semantics.md` — ADT uses data semantics
- `docs/design/0002-float-no-eq.md` — Float has no Eq ability
- `docs/design/0003-stdlib-minimal-scope.md` — Standard library tier split
- `docs/design/0004-unify-propagation-operator.md` — `?` works on Option and Result
- `docs/design/0005-multi-constraint-generics.md` — Up to 2 generic constraints
- `AGENTS.md` — AI coding assistant guidance
- `examples/` — Phase 0a example programs (01-hello.tyra through 10-data-modeling.tyra)

### External references

- Crystal documentation: <https://crystal-lang.org/reference/latest/>
- Crystal concurrency (experimental parallelism): <https://crystal-lang.org/reference/latest/guides/concurrency.html>
- Go documentation: <https://go.dev/doc/>
- Gleam: <https://gleam.run/>
- V: <https://vlang.io/>
- Ruby: <https://www.ruby-lang.org/>

### Third-party analyses (Phase 0b)

- `examples/comparisons/ANALYSIS.md` — Detailed feature comparison across 5 languages
- `examples/comparisons/gleam/` — Same 10 example programs in Gleam
- `examples/comparisons/v/` — Same 10 example programs in V
- `examples/comparisons/ruby/` — Same 10 example programs in Ruby
- `examples/comparisons/crystal/` — Same 10 example programs in Crystal

---

*This document was developed through extensive collaborative analysis between the project lead and AI assistants over multiple sessions. It represents the consolidated strategic position of the Tyra project as of April 2026.*
