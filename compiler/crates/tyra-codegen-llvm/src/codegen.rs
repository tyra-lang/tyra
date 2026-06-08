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
