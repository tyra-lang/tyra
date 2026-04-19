# ADR 0007: Use Boehm GC as the v0.1 reference garbage collector

- **Status**: Accepted
- **Date**: 2026-04-19
- **Spec sections affected**: §15

## Context

Spec §15.1 states that the reference implementation ships with a tracing GC
optimized for low pause times. Until now, the LLVM backend emitted raw
`malloc` calls for every heap allocation (data types, `List` buffers,
interpolated string buffers, argv copies) with **no `free` anywhere**. Any
non-trivial program leaked memory continuously, making the compiler unfit
for long-running workloads and blocking the roadmap items that depend on
memory reclamation (async runtime, benchmarks, stdlib expansion).

We need a working collector now. Writing a precise GC from scratch is a
multi-month project and off the critical path for reaching a usable v0.1.

## Decision

Use the **Boehm-Demers-Weiser conservative collector** (`libgc`, Homebrew
`bdw-gc`, Debian `libgc-dev`) as the v0.1 reference GC. All heap
allocations go through `GC_malloc`; `GC_init` is called once at `@main`
entry. `clang` links the produced binary against `-lgc`.

No finalizers are emitted (spec §12.3 notes finalizers are out of scope
for v0.1; `defer` handles resource release). Allocations are not yet
split into atomic/non-atomic — everything uses plain `GC_malloc`.

## Alternatives considered

- **Hand-written mark-sweep collector** — correct but multi-month effort;
  rejected as not on the critical path.
- **Reference counting** — incompatible with `data` types (which can
  form cycles, e.g. doubly-linked nodes) unless we also add cycle
  collection, which is harder than mark-sweep.
- **MMTk** — research-grade, high quality, but integration cost is
  substantial and the v0.1 codegen does not yet emit the metadata MMTk
  expects (stack maps, write barriers).
- **LLVM statepoint-based precise GC** — long-term goal, not v0.1 work.
  Leaves the door open (see "Consequences").

## Consequences

**Positive**

- Memory is reclaimed automatically; previously-leaking allocations now
  collect. Long-running programs become viable.
- Zero-cost to the developer experience — no annotations, no ownership,
  no lifetimes.
- All five `malloc` sites in codegen are replaced by a single call
  site (`GC_malloc`), so future GC replacement is a localized change.

**Negative / accepted tradeoffs**

- Boehm GC is **conservative**: any word-sized value on the stack that
  happens to look like a pointer may pin an allocation. False retention
  is possible but in practice rare for workloads of this scale.
- Adds a runtime dependency: users need `bdw-gc` installed
  (`brew install bdw-gc` on macOS, `apt install libgc-dev` on Debian).
  README documents this.
- `GC_malloc` is slower than `malloc` for tiny allocations; we trade
  allocation speed for safety. Acceptable at v0.1.

**Reversibility**

All allocation sites are centralized in the LLVM codegen layer
(`codegen.rs`, `instr_emit.rs`, `list_codegen.rs`, `builtins.rs`).
Swapping Boehm for a precise collector later is a localized change to
the extern declaration and the allocation helper; no language-visible
semantics change.

## Platform scope

macOS (Apple Silicon and Intel) and Linux are supported. Windows / MSVC
is **out of scope for v0.1**: `libgc` is available on Windows but the
driver does not currently probe for it and the overall toolchain has
not been validated on Windows. Revisit when the v0.1 release targets a
broader platform matrix.

## Future work

- `GC_malloc_atomic` for allocations known to contain no pointers
  (interpolated string buffers, `List<Int>` bodies, etc.). Needs a
  simple type classifier in codegen.
- Multi-threaded GC registration (`GC_allow_register_threads` /
  `GC_register_my_thread`) when the async runtime becomes truly
  concurrent (M9).
- Precise GC via LLVM statepoints once we have the cycles to pay for
  write barriers and stack maps. This ADR does not lock out that path.
