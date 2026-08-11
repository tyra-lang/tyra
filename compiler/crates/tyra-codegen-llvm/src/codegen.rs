use std::path::Path;

use tyra_mir::Program;
use tyra_types::Ty;

/// Struct type metadata for codegen.
#[allow(dead_code)]
pub(crate) struct StructInfo {
    /// LLVM type name: "%struct.Point"
    pub(crate) llvm_name: String,
    /// Field types in declaration order.
    pub(crate) field_types: Vec<Ty>,
    /// Whether this is an ADT tagged struct (Option/Result).
    pub(crate) is_adt: bool,
    /// true = data type, heap-allocated and passed as ptr (§8.6 reference semantics).
    pub(crate) is_data: bool,
    /// Per-field "recursive self-reference" flag for ADTs. A true entry
    /// instructs codegen to emit the field as an opaque `ptr` (GC-heap
    /// box) rather than the structural LLVM type.
    pub(crate) recursive_fields: Vec<bool>,
}

/// Function signature for cross-function type resolution.
#[allow(dead_code)]
pub(crate) struct FnSig {
    pub(crate) param_types: Vec<Ty>,
    pub(crate) return_type: Ty,
}

/// Generate LLVM IR text with coverage instrumentation (non-debug build).
/// Returns `(llvm_ir, covmap_text)`.  The covmap text must be written to
/// `<output_binary>.tyra-covmap` by the caller (e.g. the driver).
pub fn emit_llvm_ir_coverage(program: &Program) -> (String, String) {
    crate::inkwell_codegen::emit_inkwell_coverage(program)
}

/// Generate LLVM IR text with DWARF debug info (ADR-0014 §4a).
/// Use for debug (non-release) builds to enable lldb breakpoints and step.
pub fn emit_llvm_ir_debug(program: &Program) -> String {
    crate::inkwell_codegen::emit_inkwell_debug(program)
}

/// Generate LLVM IR text (non-coverage, non-debug build).
pub fn emit_llvm_ir(program: &Program) -> String {
    crate::inkwell_codegen::emit_inkwell(program)
}

/// Emit a native object file directly from the in-process LLVM module via
/// inkwell's `TargetMachine`, bypassing the textual-IR round trip through an
/// external `clang`/`opt` process. This is the fix for the cross-clang-version
/// bug: the compiler links against LLVM via inkwell and can emit IR syntax
/// (e.g. `getelementptr ... nuw`, LLVM 19+) that a `clang` resolved from PATH
/// may predate and fail to parse. Object emission has no such dependency —
/// `clang` is invoked only as a linker over the already-compiled `.o`.
///
/// `release` selects `-O2`-equivalent optimization (LLVM's `default<O2>` pass
/// pipeline) vs. no optimization. `debug` controls DWARF emission via the
/// shared `build_module_opts` pipeline (mirrors `emit_llvm_ir_debug`).
pub fn emit_object(
    program: &Program,
    path: &Path,
    release: bool,
    debug: bool,
) -> Result<(), String> {
    crate::inkwell_codegen::emit_inkwell_object(program, path, release, debug)
}

/// Coverage variant of [`emit_object`]: builds the covmap sidecar text the
/// same way [`emit_llvm_ir_coverage`] does, then emits the (always `-O0`,
/// non-debug) object file. Returns the covmap text on success so the caller
/// can write the `<output_binary>.tyra-covmap` sidecar.
pub fn emit_object_coverage(program: &Program, path: &Path) -> Result<String, String> {
    crate::inkwell_codegen::emit_inkwell_object_coverage(program, path)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn empty_program() -> Program {
        Program {
            functions: vec![],
            string_constants: vec![],
            struct_defs: vec![],
            source_files: vec![],
            lower_errors: vec![],
        }
    }

    /// Regression guard (host-agnostic): `emit_object` must write a real
    /// native object file — not textual IR — so linking never depends on the
    /// `clang` resolved from PATH being able to parse the LLVM version tyra
    /// links against. Accepts either ELF (Linux) or Mach-O (macOS) magic,
    /// whichever the host toolchain in CI/dev actually produces.
    #[test]
    fn emit_object_writes_a_real_object_file() {
        let path =
            std::env::temp_dir().join(format!("tyra_emit_object_test_{}.o", std::process::id()));
        let _ = std::fs::remove_file(&path);

        emit_object(&empty_program(), &path, false, false)
            .expect("emit_object should succeed for an empty program");

        let bytes = std::fs::read(&path).expect("object file should have been written");
        let _ = std::fs::remove_file(&path);

        assert!(!bytes.is_empty(), "object file must not be empty");
        assert!(bytes.len() >= 4, "object file too short to check magic");
        let magic = &bytes[0..4];

        // Object emission targets `TargetMachine::get_default_triple()` (the
        // host triple), so the emitted magic must match *this host's*
        // native object format specifically, not just "any format we've
        // ever seen" — otherwise a triple/format mismatch (e.g. the module
        // triple drifting from the host while the target machine is still
        // built from `module.get_triple()`) would pass this test silently.
        if cfg!(target_os = "macos") {
            // Mach-O 64-bit magic, either byte order (0xfeedfacf / 0xcffaedfe).
            assert!(
                magic == [0xcf, 0xfa, 0xed, 0xfe] || magic == [0xfe, 0xed, 0xfa, 0xcf],
                "expected Mach-O magic on macOS, got: {magic:02x?}"
            );
        } else if cfg!(target_os = "linux") {
            assert_eq!(
                magic,
                [0x7f, b'E', b'L', b'F'],
                "expected ELF magic on Linux, got: {magic:02x?}"
            );
        } else if cfg!(target_os = "windows") {
            // Plain COFF object (not a PE image, so no "MZ" header): the
            // first two bytes are the IMAGE_FILE_MACHINE_* code,
            // little-endian (0x8664 = amd64, 0xAA64 = arm64).
            let machine = &bytes[0..2];
            assert!(
                machine == [0x64, 0x86] || machine == [0x64, 0xaa],
                "expected COFF machine-type magic on Windows, got: {machine:02x?}"
            );
        } else {
            panic!(
                "unhandled host OS for object-magic assertion; extend this test \
                 for the new target instead of silently accepting any format"
            );
        }
    }
}
