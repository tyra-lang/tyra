# Contributing to Tyra

Thank you for your interest in Tyra. This document explains how to contribute effectively at this early stage of the project.

If you are an AI coding assistant (Claude Code, Codex, Cursor, etc.), please read [AGENTS.md](AGENTS.md) instead — it contains the technical rules for code generation. This document is for humans.

---

## Project status

Tyra is at **v0.1.0 — initial release**. The language specification is at v0.1, the compiler is functional, and the core feature set is stable. Expect breaking changes before v1.0.

What this means for contributors:

- The core architecture is settled, but details will change — large refactors may conflict with in-progress work
- The maintainer reserves the right to reject contributions that conflict with the design vision
- Most valuable contributions right now are **specification feedback**, **example programs**, and **bug reports** — the compiler is functional and bugs are actionable

If you are looking for a stable project to contribute to, please come back after v1.0.

---

## Ways to contribute

In rough order of current value:

### 1. Read the specification and report issues

This is the single most valuable contribution right now.

Read [docs/spec/ja/language-spec.md](docs/spec/ja/language-spec.md) (or the [English translation](docs/spec/en/language-spec.md)) and look for:

- **Ambiguities**: places where two different interpretations are possible
- **Contradictions**: rules that conflict with each other
- **Gaps**: situations the spec doesn't address
- **Inconsistencies with stated goals**: places where the spec violates §1 (e.g., introduces ambiguity, breaks AI-friendliness)

Open an issue with the `spec` label and quote the section number.

### 2. Write example programs

The `bench/static-corpus/` directory contains "spec by example" — real programs that exercise the language. Writing more of these reveals spec gaps before they become bugs.

Good targets:

- A simple HTTP API (`GET /users/:id` returning JSON)
- A CLI tool with subcommands and config file parsing
- A JSON parser (recursive ADT + error handling stress test)
- A simple ORM-like query builder
- A template engine

You don't need a working compiler — write the program *as you imagine Tyra should work*, and submit it. Mismatches between your intuition and the spec are valuable data.

### 3. Translate documentation

The specification's authoritative version is Japanese. The English translation may lag. Pull requests improving the English version are welcome, but please:

- Cross-reference with the Japanese version
- Preserve technical accuracy over fluency
- Mark sections that differ between languages with a note

### 4. Improve design documentation

The `docs/design/` directory contains Architecture Decision Records (ADRs) explaining *why* certain choices were made. If you understand a design decision after reading the code or spec, write an ADR for it.

ADRs are short (1-2 pages) and follow this template:

```markdown
# ADR NNNN: Short title

- Status: Accepted | Superseded
- Date: YYYY-MM-DD

## Context
What problem are we solving?

## Decision
What did we decide?

## Consequences
What are the tradeoffs?

## Alternatives considered
What did we reject and why?
```

### 5. Compiler code contributions

Code contributions are accepted but with caveats:

- **Discuss before implementing**: open an issue first to confirm the approach
- **Small PRs only**: changes touching multiple crates are likely to conflict with the maintainer's in-progress work
- **Tests required**: every feature needs a conformance test in `bench/static-corpus/`
- **Spec compliance is non-negotiable**: code that violates the spec will be rejected, even if it works

Areas currently accepting contributions:

- ✅ Diagnostic message clarity (English wording)
- ✅ Test infrastructure and conformance corpus
- ✅ Standard library (`stdlib/`) — new functions, bug fixes
- ⚠️ Compiler internals (lexer, parser, types, codegen): coordinate via issue first
- ❌ Language spec changes without an accepted RFC

---

## Specification changes

The specification is the source of truth for Tyra. Changing it requires a deliberate process.

### Small clarifications (typos, wording)

Open a PR directly. The maintainer will merge if the meaning is preserved. These become spec patch versions (`v0.1.0` → `v0.1.1`).

### New features or semantic changes

These require an RFC. Do not open a PR that changes spec semantics without an accepted RFC first.

**RFC process**:

1. Open an issue with the `rfc-proposal` label describing the problem
2. Wait for maintainer feedback (typically within 1-2 weeks)
3. If encouraged, draft an RFC in `docs/rfcs/` following the template
4. Open a PR with the RFC; discussion happens in the PR
5. After acceptance, the RFC is merged with status `Accepted`
6. Implementation can begin; spec is updated as part of the implementing PR

RFCs may target the next minor spec version (e.g., propose for v0.2 while v0.1 is current).

---

## Development setup

### Requirements

- Rust 1.88 or later (the project's MSRV)
- LLVM 21 and Boehm GC (`bdw-gc` / `libgc-dev`)
- Git

### Platform-specific installation

**macOS**:

```bash
brew install llvm@21 bdw-gc
echo 'export PATH="/opt/homebrew/opt/llvm@21/bin:$PATH"' >> ~/.zshrc
```

**Ubuntu/Debian**:

```bash
sudo apt install llvm-21 clang-21 libgc-dev
sudo update-alternatives --install /usr/bin/llvm-config llvm-config /usr/bin/llvm-config-21 100
```

**Windows**: Build via WSL2 is recommended. Native Windows support is not currently tested.

### Building

```bash
git clone https://github.com/tyra-lang/tyra.git
cd tyra
cargo build -p tyra-cli
```

### Running tests

```bash
# Workspace unit tests
cargo test --workspace

# Static corpus (spec conformance)
bash bench/static-corpus/check.sh ./target/debug/tyra
```

### Running the compiler

```bash
export TYRA_STDLIB=$PWD/stdlib
./target/debug/tyra run examples/01-hello.tyra
```

---

## Pull request guidelines

### Before opening a PR

- [ ] Open an issue first for non-trivial changes
- [ ] For spec changes, ensure an RFC exists and is accepted
- [ ] Run `cargo fmt` and `cargo clippy` (CI will check)
- [ ] Run `cargo test` and ensure all tests pass
- [ ] Add corpus tests in `bench/static-corpus/` for new behavior

### PR description

Include:

- **What**: a one-sentence summary
- **Why**: which issue or RFC this addresses
- **Spec reference**: which section of the spec this implements or relates to
- **Testing**: how you verified the change

Example:

```txt
What: Implement lexer support for string interpolation
Why: Closes #42
Spec: §7.3
Testing: Added 8 conformance tests covering basic interpolation, nested
expressions, and error cases.
```

### PR size

Smaller PRs review faster. Guideline:

- < 200 lines: usually reviewed within a few days
- 200-1000 lines: discuss approach in issue first
- > 1000 lines: please split

### Review process

- One maintainer approval is sufficient for merge
- The maintainer may request changes or close PRs that conflict with the design vision
- Please don't take rejection personally — early-stage projects need tight design coherence

---

## Code style

### Rust code

- Follow `cargo fmt` defaults
- Pass `cargo clippy` without warnings
- Document public functions with `///` comments
- Reference spec sections in comments where applicable:

  ```rust
  // spec §8.6: value types are immutable
  fn check_value_field_assignment(...) -> Result<(), TypeError> {
      ...
  }
  ```

### Tyra code (in stdlib and tests)

- Will be enforced by `tyra fmt` once available
- Until then, follow the examples in the spec §21
- Use `snake_case` for functions and variables, `PascalCase` for types
- Two-space indentation
- No trailing whitespace

### Commit messages

Follow this format:

```txt
component: short imperative summary

Optional longer explanation if needed. Reference issues with #42.
Reference spec sections with §8.6.
```

Components: `lexer`, `parser`, `types`, `mir`, `codegen`, `cli`, `stdlib`, `runtime`, `docs`, `spec`, `tests`, `ci`.

Examples:

```txt
lexer: handle Unicode escapes in string literals

parser: support nested ADT patterns in match arms

Refs #87. Implements spec §10.3 nested patterns.
```

Commit messages should be in **English**. The specification and discussion can be in Japanese, but commit history should be readable internationally.

---

## Communication

### Where to ask

- **Bug or spec issue**: GitHub Issues
- **Feature proposal**: GitHub Issues with `rfc-proposal` label, then RFC
- **General question**: GitHub Discussions
- **Security issue**: see [SECURITY.md](SECURITY.md) (do not use public issues)

### Languages

- **Issues and PRs**: English preferred, Japanese acceptable
- **Code, comments, identifiers**: English only
- **Specification**: Japanese is authoritative, English is translation
- **Discussions**: either language

The maintainer is bilingual and will respond in the language you write.

### Response times

This is a side project. Expect:

- Issues: response within 1-2 weeks
- PRs: review within 1-2 weeks
- RFCs: discussion may take 1-2 months

Please be patient. Pinging is fine after two weeks of silence.

---

## What we will not accept

To save everyone time, here are contributions that will be rejected:

- **Features explicitly listed as non-goals** (spec §3, §22): macros, operator overloading, trait objects, runtime reflection, ownership system, etc. If you think one of these is essential, open an RFC and explain why the design should change.
- **"Helpful" implicit conversions**: anything that goes against §2.1 explicitness.
- **Performance optimizations that change observable semantics**: This violates the explicitness principle in §2.1. Optimizations must preserve observable behavior; if a faster implementation produces different results from the spec, the implementation is wrong, not the spec.
- **New keywords or syntax not in §5.2**: requires RFC.
- **Borrowing features from other languages "because they have it"**: Tyra is selectively designed. Justify in terms of Tyra's goals, not by analogy.
- **Mass renames or formatting changes**: unless coordinated with the maintainer in advance.
- **AI-generated PRs without human review**: see next section.

---

## On AI-assisted contributions

We use AI extensively in Tyra's development — the language was designed for human-AI collaboration, after all. AI-assisted contributions are welcome with these conditions:

- **You are responsible for the code you submit.** "The AI wrote it" is not an explanation; you must understand and stand behind every line.
- **Verify spec compliance manually.** AI assistants sometimes hallucinate language features. Cross-check against the spec.
- **Don't submit AI-generated text in issues or PRs without editing.** Verbose, formulaic LLM output wastes reviewer time. Edit for clarity and brevity.
- **Disclose if a contribution is substantially AI-generated.** A note like "drafted with Claude, reviewed and tested by me" in the PR description is appreciated.
- **Read [AGENTS.md](AGENTS.md) if you're using an AI agent.** It contains the project-specific rules your AI should follow.

---

## Code of conduct

Be respectful. Disagree with ideas, not people. Assume good faith.

Concrete expectations:

- Critique technical decisions vigorously; critique people never
- If a contribution is rejected, the maintainer will explain why; please accept the decision gracefully
- Personal attacks, harassment, or discriminatory language will result in a ban
- Off-topic political or ideological discussion does not belong in this project

If you experience or witness inappropriate behavior, contact the maintainer privately via the email listed in the repository.

---

## Recognition

Significant contributors will be added to a `CONTRIBUTORS.md` file. There is no formal contributor agreement; by submitting code, you agree to license it under Apache-2.0 (the project license).

---

## License

By contributing to Tyra, you agree that your contributions will be licensed under the Apache License 2.0. See [LICENSE](LICENSE).
