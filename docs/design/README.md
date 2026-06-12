# Design Decisions

This directory contains **Architecture Decision Records (ADRs)** for the Tyra language project.

## What is an ADR?

An ADR is a short document that captures a significant design decision along with its context, rationale, and consequences. The goal is to preserve **why** a decision was made, not just **what** was decided.

The language specification (`docs/spec/`) defines what Tyra is. ADRs explain why it is that way.

## When to write an ADR

Write an ADR when:

- A design choice has meaningful alternatives that were considered and rejected
- The decision affects multiple sections of the specification or multiple compiler crates
- Someone is likely to ask "why not do it the other way?" in the future
- The decision is non-obvious or counterintuitive

Do not write an ADR for:

- Trivial choices with no realistic alternatives
- Implementation details that don't affect the language semantics
- Temporary decisions that will be revisited in the next spec version

## Format

Each ADR follows this template:

```markdown
# ADR NNNN: Short title

- **Status**: Proposed | Accepted | Superseded by ADR-NNNN
- **Date**: YYYY-MM-DD
- **Spec sections affected**: §X.Y, §Z.W

## Context

What problem are we solving? What constraints exist?

## Decision

What did we decide?

## Consequences

What are the tradeoffs? What becomes easier? What becomes harder?

## Alternatives considered

What did we reject and why?
```

## Numbering

ADRs are numbered sequentially: `0001`, `0002`, etc. Numbers are never reused. If a decision is reversed, the original ADR is marked `Superseded by ADR-NNNN` and a new ADR is created.

## Index

| ADR | Title | Status | Date |
| -- | -- | -- | -- |
| [0001](0001-adt-data-semantics.md) | ADT uses data (reference) semantics | Accepted | 2026-04-15 |
| [0002](0002-float-no-eq.md) | Float does not have the Eq ability | Accepted | 2026-04-15 |
| [0003](0003-stdlib-minimal-scope.md) | Standard library minimal scope for v0.1 | Accepted | 2026-04-15 |
| [0004](0004-unify-propagation-operator.md) | Remove `or return` and extend `?` to Option | Accepted | 2026-04-15 |
| [0005](0005-multi-constraint-generics.md) | Allow up to two constraints per type parameter | Accepted | 2026-04-15 |
| [0006](0006-top-level-expressions.md) | Allow top-level expressions as implicit main | Accepted | 2026-04-15 |
| [0007](0007-boehm-gc-reference-impl.md) | Use Boehm GC as the v0.1 reference garbage collector | Accepted | 2026-04-19 |
| [0008](0008-test-runner.md) | Test runner design (v0.2) | Accepted | 2026-05-19 |
| [0009](0009-project-manifest.md) | Project manifest and package namespace (v0.3) | Accepted | 2026-05-19 |
| [0010](0010-dependency-resolution.md) | Import resolution and dependency lookup (v0.3) | Accepted | 2026-05-19 |
| [0011](0011-closure-representation.md) | Closure representation and lambda C ABI (v0.4) | Accepted | 2026-05-21 |
| [0012](0012-panic-semantics.md) | Panic semantics and panic-expectation signaling (v0.6) | Accepted | 2026-05-26 |
| [0013](0013-test-name-syntax.md) | `test "name"` language syntax (v0.6) | Accepted | 2026-05-26 |
| [0014](0014-source-location-and-debug-info.md) | Source-location threading and debug info / DWARF (v0.6) | Accepted | 2026-05-26 |
| [0015](0015-generic-collections.md) | Generic collections — `Map<K,V>` and `Set<T>` (v0.6) | Accepted | 2026-05-26 |
| [0016](0016-persistent-collections.md) | Persistent collections — HAMT-based `Map<K,V>` and `Set<T>` (v0.7) | Accepted | 2026-05-27 |
| [0017](0017-diagnostic-quality.md) | Diagnostic quality for type-mismatch errors E0308 (v0.7) | Accepted | 2026-05-27 |
| [0018](0018-inkwell-migration.md) | Inkwell migration for type-safe LLVM IR generation (v0.7) | Accepted | 2026-05-27 |
| [0019](0019-linked-collections.md) | Insertion-ordered collections — `LinkedMap<K,V>` and `LinkedSet<T>` (v0.8) | Accepted | 2026-06-09 |
| [0020](0020-hm-inference.md) | Hindley-Milner type inference, rank-1 (v0.8) | Accepted | 2026-05-30 |
| [0021](0021-windows-support.md) | Windows (x64-MSVC) support (v0.8) | Accepted | 2026-06-09 |
| 0022–0024 | (gap — referenced by README/strategy for tuples / sorted collections but files were never committed; to be backfilled or re-pointed) | — | — |
| [0025](0025-source-file-extension.md) | Source file extension `.tyra` → `.ty` (v0.10) | Accepted | 2026-06-10 |
| [0026](0026-machine-readable-diagnostics.md) | Machine-readable diagnostics — `--error-format json` (v0.11) | Accepted | 2026-06-12 |
| [0027](0027-string-char-api.md) | Character-level string API (USV) and list sorting (v0.11) | Accepted | 2026-06-12 |
| [0028](0028-checker-module-signatures.md) | Type-checking imported module function calls (v0.11) | Accepted | 2026-06-12 |
| [0029](0029-err-main-semantics.md) | Runtime semantics of Err-returning `main` (v0.11) | Accepted | 2026-06-12 |
