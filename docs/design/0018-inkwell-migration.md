# ADR 0018: Inkwell migration for type-safe LLVM IR generation

- **Status**: Accepted
- **Date**: 2026-05-27
- **Spec sections affected**: なし（実装内部）; ツール: codegen

## Context

The codegen crate (`compiler/crates/tyra-codegen-llvm/src/`, ~7,550 LOC across 9 files) generates LLVM IR as plain text strings via `writeln!` into a `&mut String`. This approach has fundamental fragility:

- **DWARF metadata** is emitted as hand-written `!N = !DI*(...)` strings (dwarf.rs:281 lines), with no compiler type-safety to prevent malformed node definitions.
- **Debug location attachment** uses string surgery via `patch_dbg_on_last_instruction` (dwarf.rs:258), which scans IR text to append `!dbg !N` to the last instruction — a fragile, error-prone pattern.
- **Coverage instrumentation** (coverage.rs) manually emits `getelementptr` and `atomicrmw` instructions as text strings, susceptible to syntax errors and LLVM version mismatches.

These issues are inherited from ADR 0014 (source-location threading and debug info), which deferred DWARF tooling to this release. Since v0.6.0 hand-wrote DWARF metadata in text form to ship within schedule, v0.7.0 now addresses the underlying technical debt.

**inkwell** is a type-safe Rust API over LLVM's C API, maintained on crates.io. It provides:
- `module::Module` and `builder::Builder` for IR construction
- `debug_info::DIBuilder` for DWARF metadata generation
- Automatic SSA naming and instruction sequencing
- Per-LLVM-version feature flags (`llvm19-1`, `llvm21-1`, etc.) matching the CI matrix

## Decision

### 1. Add `inkwell` dependency

Add `inkwell` to `compiler/crates/tyra-codegen-llvm/Cargo.toml`:

```toml
[dependencies]
inkwell = { version = "0.21", features = ["llvm21-1"] }
```

The version `0.21` matches inkwell's Cargo crate releases; the feature flag selects the LLVM C API version.

### 2. LLVM version handling

CI currently uses:
- **Linux (Ubuntu, apt-get)**: LLVM 21 (release-gate.yml:52)
- **macOS (Homebrew)**: LLVM 21 (CI setup assumes standard Homebrew version)
- **Alpine (musl)**: LLVM 19 (musl-specific constraints, verified in CI)

**Build-time configuration**:

Create a `build.rs` script in `tyra-codegen-llvm/` that detects the LLVM version at compile time and selects the correct inkwell feature:

```rust
// build.rs
fn main() {
    let llvm_version = std::process::Command::new("llvm-config")
        .arg("--version")
        .output()
        .expect("llvm-config not found");
    
    let version_str = String::from_utf8(llvm_version.stdout)
        .expect("Invalid UTF-8 from llvm-config");
    let major_version: u32 = version_str
        .split('.')
        .next()
        .and_then(|s| s.parse().ok())
        .expect("Cannot parse LLVM version");
    
    match major_version {
        19 => println!("cargo:rustc-cfg=feature=\"llvm19\""),
        21 => println!("cargo:rustc-cfg=feature=\"llvm21\""),
        v => panic!("Unsupported LLVM version: {}", v),
    }
}
```

**Cargo.toml conditional compilation**:

```toml
[dependencies]
inkwell = { version = "0.21", features = ["llvm21-1"] }

[dev-dependencies]
# For feature-gated tests, if needed

[build-dependencies]
# build.rs itself has no extra deps
```

Alternatively, use environment-variable-based feature selection for CI (`RUSTFLAGS` in GitHub Actions), but build.rs auto-detection is preferred for local developer experience.

### 3. Migration scope for v0.7.0

**Phase 1: IR emission replacement**

Replace all `writeln!`-based IR generation in the following modules:

- `codegen.rs`: Replace `writeln!(ir, ...)` with `builder.build_*()` calls for instruction emission.
- `instr_emit.rs`: Convert instruction emission logic to use `InstructionValue` API.
- `list_codegen.rs`, `coverage.rs`, `builtins.rs`: Replace manual string instruction construction with `module.add_global`, `builder.build_*`.

**Phase 2: DWARF metadata generation**

Replace `dwarf.rs` with inkwell's `DIBuilder`:

```rust
use inkwell::debug_info::DIBuilder;

let di_builder = DIBuilder::new(&module, true, &cu);
let file = di_builder.create_file("main.tyra", ".");
let unit = di_builder.create_compile_unit(
    /* scope */ file,
    /* producer */ "tyra 0.7.0",
    /* is_optimized */ false,
    /* flags */ "",
    /* runtime_version */ 0,
);
```

This eliminates the hand-written node ID management and `defs: Vec<(u32, String)>` bookkeeping.

**Phase 3: Debug location attachment**

Replace `patch_dbg_on_last_instruction` string surgery with:

```rust
instr.set_debug_location(&ctx, debug_location);
```

The `InstructionValue::set_debug_location` API is the proper inkwell method; no text scanning required.

**Phase 4: Coverage instrumentation**

Replace manual `atomicrmw` emission in `coverage.rs`:

```rust
// Before: writeln!(ir, "%{} = atomicrmw add i64* {}, i64 1 acq_rel", counter_var, counter_addr);
// After:
let atomic_op = builder.build_atomicrmw(
    AtomicBinOp::Add,
    counter_ptr,
    context.i64_type().const_int(1, false),
    AtomicOrdering::AcqRel,
)?;
```

### 4. Clang remains the linker driver

`tyra-driver/src/lib.rs:1818` and `tyra-driver/src/lib.rs:1967` call `Command::new("clang")` as the linker/final code generator. **This is unchanged in v0.7.0.**

inkwell's `module.print_to_string()` returns the LLVM IR in text form (`.ll` format). The flow remains:

1. inkwell emits IR → `module.print_to_string()` → `.ll` file
2. Write `.ll` to `<output>.ll`
3. Call `clang <output>.ll <other-flags>` to assemble and link

**Future work (v0.8.0+)**: Switch to `TargetMachine::write_to_file(FileType::Object)` to emit `.o` directly, removing the clang dependency as a linker driver. This requires rewriting libgc, musl static linking, and symbol table machinery, so it is deferred.

### 5. Test migration

`tyra-codegen-llvm/src/lib.rs` contains ~20 inline tests doing `assert!(ir.contains("..."))` substring checks on the generated IR text. These tests will break because:

- inkwell's SSA naming differs from hand-written IR (e.g., `%1` vs `%tmp.0`)
- Whitespace and formatting differ
- Instruction ordering may change due to builder semantics

**Rewrite tests as semantic assertions** instead of IR text assertions:

```rust
// Before:
#[test]
fn test_func_emission() {
    let (ir, _) = codegen_expr("fn f() { 42 }");
    assert!(ir.contains("define i64 @f"));
}

// After:
#[test]
fn test_func_emission() {
    let module = codegen_expr("fn f() { 42 }");
    let func = module.get_function("f").expect("function f not found");
    assert_eq!(func.count_basic_blocks(), 1);
    assert!(func.get_type().get_return_type().is_some()); // i64
}
```

**No insta snapshot tests are introduced** — they would couple tests to IR layout churn and add maintenance burden.

### 6. Removal of string-surgery patterns

`dwarf.rs:258` `patch_dbg_on_last_instruction` (scans IR text, appends `!dbg !N` to last instruction) is eliminated entirely. The `set_debug_location` API handles debug location attachment properly.

## Alternatives considered

### A. Keep text IR forever

**Cost**: Cheapest implementation, no rewrite required.
**Tradeoff**: Fragility persists. DWARF metadata and coverage instrumentation remain error-prone. String surgery approaches cannot scale to optimization passes or LTO, and future maintainers inherit accumulated technical debt.
**Conclusion**: Rejected; technical debt would compound.

### B. Switch to TargetMachine::write_to_file and remove clang

**Cost**: Requires rewriting the entire linker flag machinery (libgc integration, musl `-static`, symbol table construction, etc.). High effort for v0.7.0.
**Benefit**: Eliminates clang as a runtime dependency, enables direct `.o` emission.
**Conclusion**: Rejected for v0.7.0. Deferred to v0.8.0+ after codegen stabilizes.

### C. Use llvm-sys directly

**Cost**: Lower-level than inkwell, more unsafe code, manual node ID management.
**Benefit**: Lighter dependency, maximum control.
**Conclusion**: Rejected; inkwell provides the right abstraction level and is well-maintained.

### D. Inkwell (chosen)

**Pros**:
- Type-safe Rust API over LLVM C API
- Maintained crate on crates.io
- Excellent DWARF support via `DIBuilder`
- Per-LLVM-version feature flags matching CI matrix
- Enables future optimization passes and LTO

**Cons**:
- Adds an external dependency
- Requires learning inkwell API (mitigated by good docs)
- Test suite rewrite from IR text assertions to semantic assertions

**Conclusion**: Best fit for Tyra's maintenance and future roadmap.

## Consequences

### Positive

- **Type safety**: All IR construction is compile-time checked. Malformed DWARF nodes and instruction syntax errors become impossible.
- **No string surgery**: Eliminates fragile `patch_dbg_on_last_instruction` pattern. Debug location attachment is proper API call.
- **Future extensibility**: Enables optimization passes (`PassManager`), LTO, and direct object file emission in v0.8.0+.
- **Maintainability**: Reduces codegen crate LOC (fewer `writeln!` calls, cleaner type structure).
- **DWARF quality**: `DIBuilder` generates correct metadata automatically; less manual bookkeeping.

### Negative / accepted tradeoffs

- **Test rewrite**: ~20 existing tests must migrate from IR text assertions to semantic assertions. Effort: ~4 hours.
- **LLVM version management**: Build-time feature selection adds build.rs complexity. CI matrix must pass the correct feature per OS leg. If `llvm-config` is unavailable, the build fails loudly (acceptable; clang is required anyway).
- **Snapshot test temptation**: Developers may be tempted to use `insta::assert_snapshot!` on IR output. Must actively discourage this in code review (ADR documents rationale).
- **v0.7.0 では inkwell 依存追加と CI 設定のみ完了。DWARF DIBuilder および `writeln!` → builder API 本体移行はテキスト IR との互換性制約により v0.8.0+ へ繰越し。**

### Implementation order

1. Add `build.rs` to `tyra-codegen-llvm/` for LLVM version detection
2. Add `inkwell` dependency with feature gating
3. Rewrite `dwarf.rs` using `DIBuilder` (remove hand-written node ID logic)
4. Rewrite `codegen.rs` / `instr_emit.rs` to use `builder::Builder` (replace `writeln!` with `build_*()`)
5. Rewrite `coverage.rs` to use `module.add_global` + `builder.build_atomicrmw`
6. Update test suite: migrate IR text assertions to semantic assertions
7. Verify: `cargo test --workspace` passes; `tyra test examples/*.tyra` works
8. Update CI: Pass `--features llvm21-1` on Linux/macOS, `--features llvm19-1` on Alpine (or rely on build.rs auto-detection)

## LLVM version risk

The LLVM 19 vs 21 mismatch is the primary risk. Resolution:

- **Build-time detection** (build.rs) catches mismatches early (compile time, not runtime)
- **CI matrix** must be updated to verify correct feature selection per OS
- **Local development**: Developers with LLVM 19 installed get `llvm19-1` feature automatically; no manual override needed
- **Fallback**: If build.rs fails, error message directs to install `llvm-config` (which is part of standard LLVM distros)

No runtime crashes are possible; LLVM 19 and 21 C API differences are caught at link time if build.rs is wrong.
