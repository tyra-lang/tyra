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
use inkwell::module::{Linkage, Module};
use inkwell::targets::TargetTriple;
use inkwell::types::{BasicMetadataTypeEnum, BasicType, BasicTypeEnum, PointerType, StructType};
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

    // ---- I1: declaration phase ----

    /// Closure fat pointer `{ ptr, ptr }` (ADR-0011: fn_ptr + env_ptr). Always
    /// declared so indirect-call emission can reference it unconditionally.
    fn declare_closure_type(&mut self) {
        let st = self.ctx.opaque_struct_type("struct.__closure_fat");
        st.set_body(&[self.ptr().into(), self.ptr().into()], false);
        self.struct_types.insert("__closure_fat".into(), st);
    }

    /// Fill bodies on the opaque struct types registered by
    /// `register_struct_types`. All opaque types exist first so mutually
    /// referencing layouts resolve (recursive self-references are boxed as
    /// `ptr` via `recursive_fields`, so no infinite value nesting occurs).
    fn set_struct_bodies(&self, program: &Program) {
        for sd in &program.struct_defs {
            // ADT tag field (field 0) is i8 regardless of its MIR type.
            let is_adt = sd.fields.first().map(|(n, _)| n == "tag").unwrap_or(false);
            let fields: Vec<BasicTypeEnum<'ctx>> = sd
                .fields
                .iter()
                .enumerate()
                .map(|(i, (_, ty))| {
                    if is_adt && i == 0 {
                        self.ctx.i8_type().into()
                    } else if sd.recursive_fields.get(i).copied().unwrap_or(false) {
                        self.ptr().into()
                    } else {
                        // `ty_to_basic_type` already maps Unit -> i64 (Unit is
                        // not a valid struct field type).
                        self.ty_to_basic_type(ty)
                    }
                })
                .collect();
            if let Some(st) = self.struct_types.get(&sd.name) {
                st.set_body(&fields, false);
            }
        }
    }

    /// Add a private, unnamed_addr, null-terminated C string constant.
    fn add_cstring(&self, name: &str, s: &str) {
        let init = self.ctx.const_string(s.as_bytes(), true);
        let g = self.module.add_global(init.get_type(), None, name);
        g.set_initializer(&init);
        g.set_constant(true);
        g.set_linkage(Linkage::Private);
        g.set_unnamed_addr(true);
    }

    /// Module-level globals: argc/argv capture slots, string/source/format
    /// constants, the panic sentinel, and the null-safe zero slot.
    fn declare_globals(&self, program: &Program) {
        let i32t = self.ctx.i32_type();
        let argc = self.module.add_global(i32t, None, ".tyra.argc");
        argc.set_initializer(&i32t.const_zero());
        argc.set_linkage(Linkage::Internal);
        let argv = self.module.add_global(self.ptr(), None, ".tyra.argv");
        argv.set_initializer(&self.ptr().const_null());
        argv.set_linkage(Linkage::Internal);

        for (idx, s) in program.string_constants.iter().enumerate() {
            self.add_cstring(&format!(".str.{idx}"), s);
        }
        for (idx, path) in program.source_files.iter().enumerate() {
            self.add_cstring(&format!(".src.{idx}"), path);
        }

        // printf/snprintf format strings (literal bytes incl. newlines).
        self.add_cstring(".fmt.str", "%s");
        self.add_cstring(".fmt.int", "%ld");
        self.add_cstring(".fmt.int_ln", "%ld\n");
        self.add_cstring(".fmt.float", "%g");
        self.add_cstring(".fmt.float_ln", "%g\n");
        self.add_cstring(".fmt.panic_loc", "panic at %s:%ld:\n");
        self.add_cstring(".fmt.str_ln", "%s\n");
        // Panic sentinel (ADR-0012): distinguishes panic() from sys.exit(101).
        self.add_cstring(".str.panic_sentinel", "__TYRA_PANIC__\n");

        // Zero slot for null-safe map-get unboxing (read-only).
        let i64t = self.ctx.i64_type();
        let zero = self.module.add_global(i64t, None, ".tyra_zero_slot");
        zero.set_initializer(&i64t.const_zero());
        zero.set_constant(true);
        zero.set_linkage(Linkage::Private);
        zero.set_unnamed_addr(true);
    }

    /// Declare the libc / Boehm GC / Tyra runtime externs. Data-driven from a
    /// single table to keep the ABI signatures centralized and reduce
    /// transcription error vs ~70 hand-written `declare` lines.
    fn declare_externs(&self) {
        use ExternKind::*;
        // (name, return, params, varargs)
        let externs: &[(&str, ExternKind, &[ExternKind], bool)] = &[
            ("puts", I32, &[P], false),
            ("printf", I32, &[P], true),
            ("snprintf", I32, &[P, I64, P], true),
            ("dprintf", I32, &[I32, P], true),
            ("GC_malloc", P, &[I64], false),
            ("GC_init", V, &[], false),
            ("tyra_rt_init", V, &[], false),
            ("tyra_task_spawn", P, &[P, P], false),
            ("tyra_task_await", P, &[P], false),
            ("tyra_task_select", P, &[P, I64], false),
            ("tyra_fs_read", P, &[P], false),
            ("tyra_fs_errno", I32, &[], false),
            ("tyra_fs_errmsg", P, &[], false),
            ("tyra_fs_write", V, &[P, P], false),
            ("tyra_fs_exists", I32, &[P], false),
            ("tyra_json_parse", I64, &[P], false),
            ("tyra_json_err_msg", P, &[], false),
            ("tyra_json_err_line", I64, &[], false),
            ("tyra_json_err_col", I64, &[], false),
            ("tyra_json_kind", P, &[I64], false),
            ("tyra_json_is_string", I32, &[I64], false),
            ("tyra_json_is_int", I32, &[I64], false),
            ("tyra_json_is_bool", I32, &[I64], false),
            ("tyra_json_str", P, &[I64], false),
            ("tyra_json_int", I64, &[I64], false),
            ("tyra_json_bool", I32, &[I64], false),
            ("tyra_json_get", I64, &[I64, P], false),
            ("tyra_json_at", I64, &[I64, I64], false),
            ("tyra_http_get", I64, &[P], false),
            ("tyra_http_status", I64, &[I64], false),
            ("tyra_http_body", P, &[I64], false),
            ("tyra_http_errno", I32, &[], false),
            ("tyra_http_errmsg", P, &[], false),
            ("tyra_http_server_new", P, &[], false),
            ("tyra_http_server_route", V, &[P, P, P, P], false),
            ("tyra_http_server_listen", I32, &[P, I64], false),
            ("tyra_io_read_line", P, &[], false),
            ("tyra_io_read_to_end", P, &[], false),
            ("tyra_io_eof", I32, &[], false),
            ("tyra_string_len", I64, &[P], false),
            ("tyra_string_is_empty", I32, &[P], false),
            ("tyra_string_trim", P, &[P], false),
            ("tyra_string_to_upper", P, &[P], false),
            ("tyra_string_to_lower", P, &[P], false),
            ("tyra_string_contains", I32, &[P, P], false),
            ("tyra_string_starts_with", I32, &[P, P], false),
            ("tyra_string_ends_with", I32, &[P, P], false),
            ("tyra_string_parse_int", I64, &[P], false),
            ("tyra_string_parse_errno", I32, &[], false),
            ("tyra_string_byte_at", I64, &[P, I64], false),
            ("tyra_string_substring", P, &[P, I64, I64], false),
            ("tyra_string_reverse", P, &[P], false),
            ("tyra_string_from_byte", P, &[I64], false),
            ("tyra_string_split_whitespace", V, &[P, P], false),
            ("tyra_string_split", V, &[P, P, P], false),
            ("tyra_string_replace", P, &[P, P, P], false),
            ("tyra_string_join", P, &[P, P], false),
            ("tyra_time_now_unix", I64, &[], false),
            ("tyra_time_monotonic_millis", I64, &[], false),
            ("tyra_log_info", V, &[P], false),
            ("tyra_log_warn", V, &[P], false),
            ("tyra_log_error", V, &[P], false),
            ("tyra_float_eq", I32, &[F64, F64], false),
            ("tyra_float_approx_eq", I32, &[F64, F64, F64], false),
            ("tyra_float_abs", F64, &[F64], false),
            ("tyra_float_floor", F64, &[F64], false),
            ("tyra_float_ceil", F64, &[F64], false),
            ("tyra_float_round", F64, &[F64], false),
            ("tyra_float_min", F64, &[F64, F64], false),
            ("tyra_float_max", F64, &[F64, F64], false),
            ("tyra_float_to_string", P, &[F64], false),
            ("tyra_float_parse", F64, &[P], false),
            ("tyra_float_parse_errno", I32, &[], false),
            ("tyra_float_from_int", F64, &[I64], false),
            ("tyra_float_to_int", I64, &[F64], false),
            ("tyra_float_is_nan", I32, &[F64], false),
            ("tyra_float_is_infinite", I32, &[F64], false),
            ("tyra_map_new", P, &[P, P], false),
            ("tyra_map_insert", P, &[P, P, P], false),
            ("tyra_map_remove", P, &[P, P], false),
            ("tyra_map_get", P, &[P, P], false),
            ("tyra_map_contains", I32, &[P, P], false),
            ("tyra_map_contains_key", I32, &[P, P], false),
            ("tyra_map_len", I64, &[P], false),
            ("tyra_map_for_each", V, &[P, P, P], false),
            ("tyra_hash_cstr", I64, &[P], false),
            ("tyra_cstr_eq", I32, &[P, P], false),
            ("tyra_set_new", P, &[P, P], false),
            ("tyra_set_insert", P, &[P, P], false),
            ("tyra_set_remove", P, &[P, P], false),
            ("tyra_set_contains", I32, &[P, P], false),
            ("tyra_set_len", I64, &[P], false),
            ("tyra_set_for_each", V, &[P, P, P], false),
            ("tyra_linked_map_new", P, &[P, P], false),
            ("tyra_linked_map_insert", P, &[P, P, P], false),
            ("tyra_linked_map_remove", P, &[P, P], false),
            ("tyra_linked_map_get", P, &[P, P], false),
            ("tyra_linked_map_contains_key", I32, &[P, P], false),
            ("tyra_linked_map_len", I64, &[P], false),
            ("tyra_linked_map_for_each", V, &[P, P, P], false),
            ("tyra_linked_set_new", P, &[P, P], false),
            ("tyra_linked_set_insert", P, &[P, P], false),
            ("tyra_linked_set_remove", P, &[P, P], false),
            ("tyra_linked_set_contains", I32, &[P, P], false),
            ("tyra_linked_set_len", I64, &[P], false),
            ("tyra_linked_set_for_each", V, &[P, P, P], false),
            ("abort", V, &[], false),
            ("exit", V, &[I32], false),
            ("strcmp", I32, &[P, P], false),
            ("__bench_clock_ns", I64, &[], false),
            ("strtoll", I64, &[P, P, I32], false),
        ];
        for (name, ret, params, varargs) in externs {
            if self.module.get_function(name).is_some() {
                continue;
            }
            let pty: Vec<BasicMetadataTypeEnum<'ctx>> =
                params.iter().map(|k| self.kind_meta(*k)).collect();
            let fn_ty = match ret {
                V => self.ctx.void_type().fn_type(&pty, *varargs),
                other => self.kind_basic(*other).fn_type(&pty, *varargs),
            };
            self.module
                .add_function(name, fn_ty, Some(Linkage::External));
        }
    }

    fn kind_basic(&self, k: ExternKind) -> BasicTypeEnum<'ctx> {
        match k {
            ExternKind::V => unreachable!("void is not a BasicTypeEnum"),
            ExternKind::I32 => self.ctx.i32_type().into(),
            ExternKind::I64 => self.ctx.i64_type().into(),
            ExternKind::F64 => self.ctx.f64_type().into(),
            ExternKind::P => self.ptr().into(),
        }
    }

    fn kind_meta(&self, k: ExternKind) -> BasicMetadataTypeEnum<'ctx> {
        self.kind_basic(k).into()
    }

    /// Declare a function signature for every program function. Bodies are
    /// emitted later (I2); I1 fills them with `unreachable` so the module
    /// verifies. `is_main` functions get the C entry signature
    /// `i32 @main(i32 %argc, ptr %argv)`; `Unit`/`Never` returns map to `void`.
    fn declare_functions(&mut self, program: &Program) {
        for f in &program.functions {
            let fn_ty = if f.is_main {
                let i32t = self.ctx.i32_type();
                i32t.fn_type(&[i32t.into(), self.ptr().into()], false)
            } else {
                let params: Vec<BasicMetadataTypeEnum<'ctx>> = f
                    .params
                    .iter()
                    .map(|(_, ty)| self.ty_to_basic_type(ty).into())
                    .collect();
                match &f.return_type {
                    Ty::Unit | Ty::Never => self.ctx.void_type().fn_type(&params, false),
                    ret => self.ty_to_basic_type(ret).fn_type(&params, false),
                }
            };
            let llvm_name = if f.is_main { "main" } else { f.name.as_str() };
            let fv = self.module.add_function(llvm_name, fn_ty, None);
            self.fn_values.insert(f.name.clone(), fv);
        }
    }

    /// I1: fill every declared function with a single `unreachable` entry block
    /// so `Module::verify()` passes. I2 replaces these with real bodies.
    fn emit_unreachable_bodies(&self) {
        for fv in self.fn_values.values() {
            let entry = self.ctx.append_basic_block(*fv, "entry");
            self.builder.position_at_end(entry);
            self.builder.build_unreachable().unwrap();
        }
    }
}

/// Compact extern signature kinds for the data-driven `declare_externs` table.
#[derive(Clone, Copy)]
enum ExternKind {
    V,
    I32,
    I64,
    F64,
    P,
}

/// Build a module from `program` via the inkwell backend and return its IR text.
///
/// **I1**: emits the full declaration surface — struct bodies, the closure fat
/// pointer type, module globals (argc/argv, string/source/format constants,
/// panic sentinel, zero slot), runtime externs, and function signatures —
/// then fills each function with a single `unreachable` entry block so the
/// module verifies. Real instruction bodies, builtins, coverage and DWARF land
/// in I2–I6. Not yet wired into the public `emit_llvm_ir*` entry points (the
/// legacy text path remains production until I7).
#[allow(dead_code)]
pub(crate) fn emit_inkwell(program: &Program) -> String {
    let ctx = Context::create();
    build_module(&ctx, program).module.print_to_string().to_string()
}

/// Run the I1 declaration pipeline and return the populated `CodeGen` (module
/// not yet finalized to text). Shared by `emit_inkwell` and tests that need to
/// run `Module::verify()` on the result.
#[allow(dead_code)]
fn build_module<'ctx>(ctx: &'ctx Context, program: &Program) -> CodeGen<'ctx> {
    let mut cg = CodeGen::new(ctx, "tyra");
    cg.register_struct_types(program);
    cg.declare_closure_type();
    cg.set_struct_bodies(program);
    cg.declare_globals(program);
    cg.declare_externs();
    cg.declare_functions(program);
    cg.emit_unreachable_bodies();
    cg
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

    fn sample_program() -> Program {
        use tyra_mir::Function;
        Program {
            functions: vec![
                Function {
                    name: "add".into(),
                    params: vec![("a".into(), Ty::Int), ("b".into(), Ty::Int)],
                    return_type: Ty::Int,
                    body: vec![],
                    is_main: false,
                    local_metas: vec![],
                },
                Function {
                    name: "noop".into(),
                    params: vec![],
                    return_type: Ty::Unit,
                    body: vec![],
                    is_main: false,
                    local_metas: vec![],
                },
                Function {
                    name: "main".into(),
                    params: vec![],
                    return_type: Ty::Int,
                    body: vec![],
                    is_main: true,
                    local_metas: vec![],
                },
            ],
            string_constants: vec!["hello".into()],
            struct_defs: vec![tyra_mir::StructDef {
                name: "Pair".into(),
                fields: vec![("a".into(), Ty::Int), ("b".into(), Ty::Int)],
                is_data: false,
                recursive_fields: vec![false, false],
            }],
            source_files: vec!["main.tyra".into()],
            lower_errors: vec![],
        }
    }

    #[test]
    fn i1_declarations_module_verifies() {
        let ctx = Context::create();
        let cg = build_module(&ctx, &sample_program());
        assert!(
            cg.module.verify().is_ok(),
            "module failed to verify:\n{}",
            cg.module.print_to_string().to_string()
        );
    }

    #[test]
    fn i1_emits_expected_declarations() {
        let ir = emit_inkwell(&sample_program());
        // main entry signature
        assert!(ir.contains("define i32 @main(i32"), "missing @main:\n{ir}");
        // NOTE: named struct types (%struct.Pair, %struct.__closure_fat) are
        // intentionally NOT asserted here — LLVM elides unreferenced named
        // structs from the textual IR, and I1 bodies are `unreachable` so
        // nothing references them yet. Their definition is validated by
        // `Module::verify()` (i1_declarations_module_verifies) and the type
        // bridge tests; they reappear in the IR once I2 emits instructions.
        // runtime/libc externs
        assert!(ir.contains("@printf"));
        assert!(ir.contains("@GC_malloc"));
        assert!(ir.contains("@tyra_rt_init"));
        // globals / format constants
        assert!(ir.contains("@.tyra.argc"));
        assert!(ir.contains("@.fmt.int"));
        assert!(ir.contains("@.str.0")); // "hello"
        // Unit-returning fn lowers to void
        assert!(ir.contains("@noop"));
    }
}
