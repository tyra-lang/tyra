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

/// A struct type definition (from value/data types).
#[derive(Debug, Clone)]
pub struct StructDef {
    pub name: String,
    pub fields: Vec<(String, Ty)>,
}

/// A complete MIR program.
#[derive(Debug, Clone)]
pub struct Program {
    pub functions: Vec<Function>,
    /// String constants used in the program.
    pub string_constants: Vec<String>,
    /// Struct type definitions for value/data types.
    pub struct_defs: Vec<StructDef>,
}

/// A MIR function.
#[derive(Debug, Clone)]
pub struct Function {
    pub name: String,
    pub params: Vec<(String, Ty)>,
    pub return_type: Ty,
    pub body: Vec<Instruction>,
    pub is_main: bool,
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
    And,
    Or,
}
