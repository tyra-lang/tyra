//! Inkwell-based LLVM IR generation (v0.9.0, ADR-0018 Theme A).
//!
//! **I0 scaffold.** Establishes the `CodeGen<'ctx>` value-handle model that
//! replaces the string-based IR builder in `codegen.rs`. SSA values become
//! typed `BasicValueEnum` handles keyed by MIR temp name, so a value's LLVM
//! type travels with it to every use site — structurally eliminating the
//! "default an unknown operand to i64" mistyping class the text backend was
//! prone to (see `helpers::infer_operand_type`).
//!
//! The legacy text path in `codegen.rs` remains the production backend until
//! the migration completes (I7). This module is compiled and exercised in
//! parallel so each phase (I1 declarations, I2 instructions, …) lands
//! incrementally behind its own verification gate. I0 itself changes no
//! observable output.

use std::collections::{HashMap, HashSet};

use inkwell::AddressSpace;
use inkwell::basic_block::BasicBlock;
use inkwell::builder::Builder;
use inkwell::context::Context;
use inkwell::module::Module;
use inkwell::targets::TargetTriple;
use inkwell::types::{BasicTypeEnum, PointerType, StructType};
use inkwell::values::{BasicValueEnum, FunctionValue, PointerValue};

use tyra_mir::Program;
use tyra_types::Ty;

use crate::helpers::target_triple;

/// Inkwell codegen state for one module.
///
/// Replaces the string-based value/type tracking of the legacy text backend:
/// every SSA value is a typed `BasicValueEnum` handle keyed by its MIR temp
/// name, and named types resolve through `struct_types`/`data_types` rather
/// than string lookups.
// Several fields/methods are unread in the I0 scaffold; they are populated and
// consumed from I1 (declarations) and I2 (instructions, blocks, allocas) on.
#[allow(dead_code)]
pub(crate) struct CodeGen<'ctx> {
    pub(crate) ctx: &'ctx Context,
    pub(crate) module: Module<'ctx>,
    pub(crate) builder: Builder<'ctx>,
    /// MIR temp name -> SSA value handle.
    pub(crate) values: HashMap<String, BasicValueEnum<'ctx>>,
    /// Named (and monomorphized) struct/ADT types.
    pub(crate) struct_types: HashMap<String, StructType<'ctx>>,
    /// `data` types (§8.6): heap-allocated, represented as `ptr`.
    pub(crate) data_types: HashSet<String>,
    /// Declared functions by name.
    pub(crate) fn_values: HashMap<String, FunctionValue<'ctx>>,
    /// Basic blocks by MIR label name. Reset per function (I2).
    pub(crate) blocks: HashMap<String, BasicBlock<'ctx>>,
    /// alloca slots (param/local addresses) by name. Reset per function (I2).
    pub(crate) addr_slots: HashMap<String, PointerValue<'ctx>>,
}

impl<'ctx> CodeGen<'ctx> {
    pub(crate) fn new(ctx: &'ctx Context, module_name: &str) -> Self {
        let module = ctx.create_module(module_name);
        module.set_triple(&TargetTriple::create(target_triple()));
        CodeGen {
            ctx,
            module,
            builder: ctx.create_builder(),
            values: HashMap::new(),
            struct_types: HashMap::new(),
            data_types: HashSet::new(),
            fn_values: HashMap::new(),
            blocks: HashMap::new(),
            addr_slots: HashMap::new(),
        }
    }

    /// Opaque `ptr` type (LLVM 15+ opaque pointers).
    #[allow(dead_code)] // used from I1 on (declarations, value emission)
    pub(crate) fn ptr(&self) -> PointerType<'ctx> {
        self.ctx.ptr_type(AddressSpace::default())
    }

    /// Bridge a Tyra `Ty` to an inkwell **value** type. Mirrors
    /// `helpers::llvm_type_str` but yields a typed handle.
    ///
    /// `Unit`/`Never` map to `i64` here because the legacy backend stores Unit
    /// as `i64` in value position (e.g. ADT struct fields, codegen.rs). The
    /// `void` *return* type is not a `BasicTypeEnum` and is handled separately
    /// at function-signature emission (I1).
    #[allow(dead_code)] // used from I1 on (declarations, value emission)
    pub(crate) fn ty_to_basic_type(&self, ty: &Ty) -> BasicTypeEnum<'ctx> {
        match ty {
            Ty::Int => self.ctx.i64_type().into(),
            Ty::Float => self.ctx.f64_type().into(),
            Ty::Bool => self.ctx.bool_type().into(),
            Ty::String => self.ptr().into(),
            Ty::Fn(..) => self.ptr().into(),
            Ty::Named(name) => {
                if self.data_types.contains(name) {
                    self.ptr().into()
                } else if let Some(st) = self.struct_types.get(name) {
                    (*st).into()
                } else {
                    self.ctx.i64_type().into()
                }
            }
            Ty::Generic(..) => {
                // Check `data_types` first: a monomorphized generic *data* type
                // (e.g. `data Box<Int>` → `Box__Int`) is heap-allocated and
                // represented as `ptr` (§8.6), like the `Named` branch above.
                // The monomorphized name is what `register_struct_types` inserts
                // into both maps, so the lookup key matches.
                let mono = ty.monomorphized_name();
                if self.data_types.contains(&mono) {
                    self.ptr().into()
                } else if let Some(st) = self.struct_types.get(&mono) {
                    (*st).into()
                } else {
                    self.ctx.i64_type().into()
                }
            }
            // Unit / Never / unresolved → i64 in value position.
            _ => self.ctx.i64_type().into(),
        }
    }

    /// Register an (opaque) named struct type for every struct/ADT definition
    /// so the type bridge resolves `Named`/`Generic` types. Bodies are filled
    /// in I1 (declaration phase) once field layout is computed.
    fn register_struct_types(&mut self, program: &Program) {
        for sd in &program.struct_defs {
            if sd.is_data {
                self.data_types.insert(sd.name.clone());
            }
            let st = self.ctx.opaque_struct_type(&format!("struct.{}", sd.name));
            self.struct_types.insert(sd.name.clone(), st);
        }
    }
}

/// Build a module from `program` via the inkwell backend and return its IR text.
///
/// **I0**: module skeleton (target triple) + registered struct types only.
/// Globals, externs, function signatures/bodies, builtins, coverage and DWARF
/// land in I1–I6. Not yet wired into the public `emit_llvm_ir*` entry points
/// (the legacy text path remains production until I7).
#[allow(dead_code)]
pub(crate) fn emit_inkwell(program: &Program) -> String {
    let ctx = Context::create();
    let mut cg = CodeGen::new(&ctx, "tyra");
    cg.register_struct_types(program);
    cg.module.print_to_string().to_string()
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

    #[test]
    fn emit_inkwell_produces_module_with_target_triple() {
        let ir = emit_inkwell(&empty_program());
        assert!(ir.contains("target triple"), "IR missing target triple:\n{ir}");
    }

    #[test]
    fn type_bridge_maps_primitives() {
        let ctx = Context::create();
        let cg = CodeGen::new(&ctx, "t");
        assert!(cg.ty_to_basic_type(&Ty::Int).is_int_type());
        assert!(cg.ty_to_basic_type(&Ty::Float).is_float_type());
        assert!(cg.ty_to_basic_type(&Ty::Bool).is_int_type()); // i1
        assert!(cg.ty_to_basic_type(&Ty::String).is_pointer_type());
        assert!(
            cg.ty_to_basic_type(&Ty::Fn(vec![Ty::Int], Box::new(Ty::Int)))
                .is_pointer_type()
        );
    }

    #[test]
    fn data_type_bridges_to_ptr_named_struct_to_struct() {
        let ctx = Context::create();
        let mut cg = CodeGen::new(&ctx, "t");
        let program = Program {
            functions: vec![],
            string_constants: vec![],
            struct_defs: vec![
                tyra_mir::StructDef {
                    name: "Heap".into(),
                    fields: vec![("x".into(), Ty::Int)],
                    is_data: true,
                    recursive_fields: vec![false],
                },
                tyra_mir::StructDef {
                    name: "Pair".into(),
                    fields: vec![("a".into(), Ty::Int), ("b".into(), Ty::Int)],
                    is_data: false,
                    recursive_fields: vec![false, false],
                },
            ],
            source_files: vec![],
            lower_errors: vec![],
        };
        cg.register_struct_types(&program);
        // data type → ptr
        assert!(cg.ty_to_basic_type(&Ty::Named("Heap".into())).is_pointer_type());
        // value struct → struct type
        assert!(cg.ty_to_basic_type(&Ty::Named("Pair".into())).is_struct_type());
    }

    #[test]
    fn generic_data_type_bridges_to_ptr() {
        let ctx = Context::create();
        let mut cg = CodeGen::new(&ctx, "t");
        // A monomorphized generic data type must resolve to `ptr`, not a struct.
        let boxed = Ty::Generic("Box".into(), vec![Ty::Int]);
        let mono = boxed.monomorphized_name();
        let program = Program {
            functions: vec![],
            string_constants: vec![],
            struct_defs: vec![tyra_mir::StructDef {
                name: mono,
                fields: vec![("v".into(), Ty::Int)],
                is_data: true,
                recursive_fields: vec![false],
            }],
            source_files: vec![],
            lower_errors: vec![],
        };
        cg.register_struct_types(&program);
        assert!(cg.ty_to_basic_type(&boxed).is_pointer_type());
    }
}
