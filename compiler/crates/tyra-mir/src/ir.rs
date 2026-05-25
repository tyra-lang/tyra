// Mid-level IR definitions.
//
// The MIR is a simplified representation between the AST and LLVM IR.
// It desugars control flow into basic blocks, flattens expressions into
// a sequence of instructions with explicit temporaries, and makes all
// operations explicit.
//
// Design goals:
// - Easy to translate to LLVM IR
// - Each instruction has a clear, single effect
// - Control flow is explicit (no nested expressions)

use tyra_types::Ty;

/// Source location attached to each MIR instruction.
/// Derived from the AST Span during lowering; used for DWARF, coverage,
/// and panic diagnostics (ADR 0014).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SourceLoc {
    /// Index into Program::source_files.
    pub file_id: u32,
    pub line: u32,
    pub col: u32,
}

impl SourceLoc {
    /// A placeholder location for compiler-synthesized instructions that have
    /// no corresponding source position (alloca hoists, implicit returns, etc.).
    pub const fn dummy() -> Self {
        Self {
            file_id: 0,
            line: 0,
            col: 0,
        }
    }

    pub fn is_dummy(self) -> bool {
        self.line == 0
    }
}

/// An instruction paired with its source location.
#[derive(Debug, Clone)]
pub struct MirStmt {
    pub loc: SourceLoc,
    pub instr: Instruction,
}

impl MirStmt {
    pub fn new(loc: SourceLoc, instr: Instruction) -> Self {
        Self { loc, instr }
    }

    /// Synthesized instruction with no source position (alloca hoists, etc.).
    pub fn synthetic(instr: Instruction) -> Self {
        Self {
            loc: SourceLoc::dummy(),
            instr,
        }
    }
}

/// Per-local-variable metadata collected during lowering.
/// Used to emit DWARF DILocalVariable entries (ADR 0014 §3).
#[derive(Debug, Clone)]
pub struct LocalMeta {
    pub name: String,
    pub ty: Ty,
    /// Name of the alloca slot emitted by codegen (empty for SSA-only temps).
    pub alloca_name: String,
}

/// A struct type definition (from value/data types).
#[derive(Debug, Clone)]
pub struct StructDef {
    pub name: String,
    pub fields: Vec<(String, Ty)>,
    /// true = data type (reference semantics, §8.6); false = value type or ADT
    pub is_data: bool,
    /// Per-field "is a recursive self-reference" flag. Only meaningful for
    /// ADT structs. A true entry means the field's declared type is (or
    /// transitively contains) this ADT itself; codegen emits such fields
    /// as a boxed GC-heap `ptr` to avoid the otherwise-infinite LLVM
    /// struct layout. The vector length matches `fields`.
    pub recursive_fields: Vec<bool>,
}

/// A complete MIR program.
#[derive(Debug, Clone)]
pub struct Program {
    pub functions: Vec<Function>,
    /// String constants used in the program.
    pub string_constants: Vec<String>,
    /// Struct type definitions for value/data types.
    pub struct_defs: Vec<StructDef>,
    /// Source file paths indexed by SourceLoc::file_id.
    /// file_id 0 is the primary source file; additional entries come from
    /// inlined stdlib or multi-file builds (future).
    pub source_files: Vec<String>,
}

/// A MIR function.
#[derive(Debug, Clone)]
pub struct Function {
    pub name: String,
    pub params: Vec<(String, Ty)>,
    pub return_type: Ty,
    /// Instructions with source locations (ADR 0014).
    pub body: Vec<MirStmt>,
    pub is_main: bool,
    /// Metadata for local variables / parameters, used for DWARF locals.
    pub local_metas: Vec<LocalMeta>,
}

/// An instruction in the MIR.
/// Each instruction produces a result in a named temporary (unless void).
#[derive(Debug, Clone)]
pub enum Instruction {
    /// `dest = constant`
    Const { dest: String, value: Constant },

    /// `dest = call func(args...)`
    Call {
        dest: Option<String>,
        func: String,
        args: Vec<Operand>,
    },

    /// `dest = binop lhs, rhs`
    BinOp {
        dest: String,
        op: MirBinOp,
        lhs: Operand,
        rhs: Operand,
    },

    /// `dest = neg operand`
    Neg { dest: String, operand: Operand },

    /// `dest = not operand`
    Not { dest: String, operand: Operand },

    /// `dest = copy source` (variable reference)
    Copy { dest: String, source: String },

    /// `return operand` or `return` (for Unit)
    Return { value: Option<Operand> },

    /// Label for a basic block target.
    Label(String),

    /// Conditional branch: if operand then goto true_label else goto false_label
    BranchIf {
        cond: Operand,
        true_label: String,
        false_label: String,
    },

    /// Unconditional jump
    Jump { label: String },

    /// `dest = phi [(val1, label1), (val2, label2)]`
    /// Used for if/match expression results.
    Phi {
        dest: String,
        branches: Vec<(Operand, String)>,
    },

    /// `dest = alloca` — allocate stack slot for a mutable local
    Alloca { dest: String },

    /// `store value -> dest_ptr` — write to an alloca'd slot
    Store { dest: String, value: Operand },

    /// `dest = load source_ptr` — read from an alloca'd slot
    Load { dest: String, source: String },

    /// `dest = struct_init type_name { val0, val1, ... }`
    /// Constructs a struct value from field values in declaration order.
    StructInit {
        dest: String,
        type_name: String,
        fields: Vec<Operand>,
    },

    /// `dest = extractfield obj.field_index` (from struct type_name)
    /// Extracts a field from a struct value.
    FieldGet {
        dest: String,
        obj: Operand,
        type_name: String,
        field_index: u32,
    },

    /// In-place field mutation for data types: `obj.field_index = value` (GEP + store, §8.6).
    /// `obj` must be a pointer to the struct (data type only).
    FieldSet {
        obj: Operand,
        type_name: String,
        field_index: u32,
        value: Operand,
    },

    /// `dest = adt_init type_name { tag, fields... }`
    /// Constructs an ADT variant as a tagged struct.
    /// fields: payload values in struct field order (excluding the tag at field 0).
    /// Empty for unit variants (e.g., None, Cash).
    AdtInit {
        dest: String,
        type_name: String,
        tag: i64,
        fields: Vec<Operand>,
    },

    /// `dest = adt_tag obj`
    /// Extracts the tag (field 0) from an ADT tagged struct.
    AdtTag {
        dest: String,
        obj: Operand,
        type_name: String,
    },

    /// `dest = adt_payload obj[field_index]`
    /// Extracts a payload field from an ADT tagged struct.
    /// For Option: field_index=1 (value). For Result: field_index=1 (ok) or 2 (err).
    AdtPayload {
        dest: String,
        obj: Operand,
        type_name: String,
        field_index: u32,
    },

    /// `dest = string_format(format_ref, args...)`
    /// Formats a string using a printf-style format string and arguments.
    /// Used for string interpolation outside print() calls.
    /// format_ref is an index into Program.string_constants.
    /// Result is a heap-allocated (malloc) 1024-byte buffer.
    /// Strings longer than 1024 bytes are truncated.
    /// TODO: GC integration to free these buffers.
    StringFormat {
        dest: String,
        format_ref: usize,
        args: Vec<Operand>,
    },

    /// `dest = list_init(elem_type, [e0, e1, ...])` — construct a list from elements (§11).
    /// Heap-allocates storage for elements and produces a {ptr, i64} struct.
    ListInit {
        dest: String,
        elem_type: Ty,
        elements: Vec<Operand>,
    },

    /// `dest = list_len(list)` — extract the length (field 1) from a list struct (§11).
    ListLen { dest: String, list: Operand },

    /// `dest = list_get(list, index, elem_type)` — panicking index access (§11).
    /// Aborts if index >= length.
    ListGet {
        dest: String,
        list: Operand,
        index: Operand,
        elem_type: Ty,
    },

    /// `dest = list_get_safe(list, index, elem_type)` — safe access returning Option<T> (§11).
    /// Returns Some(element) if in bounds, None otherwise.
    ListGetSafe {
        dest: String,
        list: Operand,
        index: Operand,
        elem_type: Ty,
    },

    /// `dest = list_push(list, elem, elem_type)` — immutable append (§17.3.5).
    /// Allocates a fresh buffer of (len+1) elements, copies the input, and
    /// stores `elem` at the tail. Returns a new {ptr, i64} struct. Input is
    /// never mutated. Polymorphic over `elem_type` (Int, String, Bool, ...).
    ListPush {
        dest: String,
        list: Operand,
        elem: Operand,
        elem_type: Ty,
    },

    /// `dest = map_get_option(handle, key)` — call `tyra_map_get`, check for
    /// null, and construct `Option<V>`.  Tag is 0 for Some, 1 for None.
    /// `val_ty` carries the concrete V type so codegen knows how to unbox.
    /// `key_ty` carries the concrete K type so codegen knows the boxing width.
    MapGetOption {
        dest: String,
        handle: Operand,
        key: Operand,
        key_ty: Ty,
        val_ty: Ty,
    },

    /// `dest = spawn func(args...)` — submit a task to the async runtime (§14.4, M9).
    /// Codegen emits a synthetic thunk that unboxes args, calls `func`, and boxes
    /// the result. `arg_types` and `result_type` drive the LLVM layout of the
    /// per-site argument/result boxes.
    Spawn {
        dest: String,
        func: String,
        args: Vec<Operand>,
        arg_types: Vec<Ty>,
        result_type: Ty,
    },

    /// `dest = task.await` — block on a task handle, unboxing its result (§14.3, M9).
    /// `result_type` is the T in `Task<T>` and determines how the boxed result
    /// produced by the spawn thunk is loaded.
    Await {
        dest: String,
        task: Operand,
        result_type: Ty,
    },

    /// `dest = tasks.join_all(list)` — await every task handle in `list` and
    /// produce a `List<T>` of unboxed results (§17.1, M9). Codegen inlines
    /// a loop that calls `tyra_task_await` on each handle, loads T from the
    /// returned box, and builds a fresh result list.
    JoinAll {
        dest: String,
        list: Operand,
        elem_type: Ty,
    },

    /// `dest = tasks.select(list)` — dispatch to `tyra_task_select`, which
    /// returns a new `Task<T>` handle that resolves with the first source
    /// task's result (§17.1). The dest is an i64 task handle; downstream
    /// `.await` unboxes T as usual. `elem_type` is the T the caller's
    /// `.await` will extract from the winning task's result box.
    Select {
        dest: String,
        list: Operand,
        elem_type: Ty,
    },

    /// Build a closure fat-pointer value (ADR-0011).
    /// `fn_name` — the name of the lifted LLVM function (`__lambda_N`).
    /// `env_fields` — captured operands in lexical first-use order.
    /// `env_struct_name` — name of the `__closure_env_N` StructDef; empty when
    ///   there are no captures (non-capturing lambda, env_ptr = null).
    /// `param_types` / `return_type` — the lifted function's user-visible
    ///   signature (excludes the hidden `__env: ptr` first parameter).
    ClosureBuild {
        dest: String,
        fn_name: String,
        env_fields: Vec<Operand>,
        env_struct_name: String,
        param_types: Vec<Ty>,
        return_type: Ty,
    },

    /// Indirect call through a fat-pointer closure value (ADR-0011).
    /// `fat_ptr` — operand holding the `ptr` to the `__closure_fat` struct.
    /// `args` — user-visible arguments (the hidden `env_ptr` is prepended by
    ///   codegen when emitting the LLVM `call` instruction).
    /// `param_types` / `return_type` — the lifted function's user-visible
    ///   signature, used to emit the correct LLVM call type.
    IndirectCall {
        dest: Option<String>,
        fat_ptr: Operand,
        args: Vec<Operand>,
        param_types: Vec<Ty>,
        return_type: Ty,
    },
}

/// A constant value.
#[derive(Debug, Clone)]
pub enum Constant {
    Int(i64),
    Float(f64),
    Bool(bool),
    /// Index into Program.string_constants
    StringRef(usize),
    Unit,
}

/// An operand (value reference) in an instruction.
#[derive(Debug, Clone)]
pub enum Operand {
    /// Named temporary or variable
    Var(String),
    /// Inline constant
    Const(Constant),
}

/// MIR binary operations (explicit about the type).
#[derive(Debug, Clone, Copy)]
pub enum MirBinOp {
    AddInt,
    SubInt,
    MulInt,
    DivInt,
    RemInt,
    AddFloat,
    SubFloat,
    MulFloat,
    DivFloat,
    EqInt,
    NeqInt,
    LtInt,
    LeInt,
    GtInt,
    GeInt,
    LtFloat,
    LeFloat,
    GtFloat,
    GeFloat,
    EqString,
    NeqString,
    And,
    Or,
}
