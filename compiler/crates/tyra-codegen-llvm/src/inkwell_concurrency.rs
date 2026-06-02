//! Inkwell I4i: async concurrency (`spawn` / `await` / `join_all` / `select`,
//! §14 / §17.1, M9).
//!
//! Task handles travel through the MIR as `i64` (so they flow through lists and
//! mixed-type expressions); codegen round-trips through `ptr` at the runtime
//! boundary via `ptrtoint`/`inttoptr` (64-bit flat pointers — CHERI / 32-bit are
//! out of scope, same assumption as the legacy backend).
//!
//! `spawn func(args)` is the involved case: each site needs a synthetic
//! `@__tyra_spawn_thunk_N(ptr args) -> ptr` matching the C ABI `tyra_task_spawn`
//! expects, plus a per-site `__tyra_spawn_args_N` struct that boxes the
//! arguments. Both are pre-declared by `declare_spawn_thunks` (in program order,
//! so the id a `Spawn` site uses is fixed regardless of which functions later
//! fall back to `unreachable`); the thunk *bodies* are filled after all user
//! bodies by `emit_spawn_thunk_bodies` (the thunk calls the user function, which
//! must already be defined). Mirrors legacy `emit_spawn`/`emit_spawn_thunk`/
//! `emit_await`/`emit_join_all`/`emit_select` (instr_emit.rs / codegen.rs).
//!
//! `GC_malloc` OOM branches are omitted (Boehm aborts internally), consistent
//! with the rest of the inkwell migration.

use inkwell::IntPredicate;
use inkwell::module::Linkage;
use inkwell::types::{BasicType, BasicTypeEnum};
use inkwell::values::BasicMetadataValueEnum;

use tyra_mir::{Instruction, Operand, Program};
use tyra_types::Ty;

use crate::inkwell_codegen::CodeGen;

/// Descriptor for one `spawn` site's synthetic thunk (mirrors legacy
/// `codegen::SpawnThunk`). The index in `CodeGen.spawn_thunks` is the id.
#[derive(Clone)]
pub(crate) struct SpawnThunkDesc {
    pub(crate) id: usize,
    pub(crate) func: String,
    pub(crate) arg_types: Vec<Ty>,
    pub(crate) result_type: Ty,
}

impl<'ctx> CodeGen<'ctx> {
    /// Pre-declare every `spawn` site's argument struct (`__tyra_spawn_args_N`)
    /// and thunk function (`@__tyra_spawn_thunk_N`, `ptr(ptr)`, internal
    /// linkage) in program order, and record `spawn_bases[fi]` so emission can
    /// reset the id cursor per function. Bodies are emitted later by
    /// `emit_spawn_thunk_bodies`.
    pub(crate) fn declare_spawn_thunks(&mut self, program: &Program) {
        let ptr = self.ptr();
        for f in &program.functions {
            self.spawn_bases.push(self.spawn_thunks.len());
            for stmt in &f.body {
                if let Instruction::Spawn { func, arg_types, result_type, .. } = &stmt.instr {
                    let id = self.spawn_thunks.len();
                    // Per-site argument struct (boxes the spawn arguments).
                    let field_tys: Vec<BasicTypeEnum<'ctx>> =
                        arg_types.iter().map(|t| self.ty_to_basic_type(t)).collect();
                    let args_st = self.ctx.struct_type(&field_tys, false);
                    self.struct_types.insert(format!("__tyra_spawn_args_{id}"), args_st);
                    // Thunk function signature `ptr(ptr args)`.
                    let fn_ty = ptr.fn_type(&[ptr.into()], false);
                    let name = format!("__tyra_spawn_thunk_{id}");
                    let fv = self.module.add_function(&name, fn_ty, Some(Linkage::Internal));
                    self.fn_values.insert(name, fv);
                    self.spawn_thunks.push(SpawnThunkDesc {
                        id,
                        func: func.clone(),
                        arg_types: arg_types.clone(),
                        result_type: result_type.clone(),
                    });
                }
            }
        }
    }

    /// `dest = spawn func(args...)` — box args into the per-site struct, call
    /// `tyra_task_spawn(thunk, args)`, and bind the handle (as `i64`) to `dest`.
    pub(crate) fn emit_spawn(
        &mut self,
        dest: &str,
        _func: &str,
        args: &[Operand],
        _arg_types: &[Ty],
        _result_type: &Ty,
    ) {
        let id = self.spawn_cursor;
        self.spawn_cursor += 1;
        let ptr = self.ptr();
        let i64t = self.ctx.i64_type();

        let args_box: BasicMetadataValueEnum<'ctx> = if args.is_empty() {
            ptr.const_null().into()
        } else {
            let args_st = self.struct_types[&format!("__tyra_spawn_args_{id}")];
            let size = args_st.size_of().expect("spawn args struct is sized");
            let gc = self.module.get_function("GC_malloc").unwrap();
            let abox = self
                .builder
                .build_call(gc, &[size.into()], &format!("{dest}.args"))
                .unwrap()
                .try_as_basic_value()
                .basic()
                .unwrap()
                .into_pointer_value();
            for (i, arg) in args.iter().enumerate() {
                let v = self.operand(arg);
                let gep = self
                    .builder
                    .build_struct_gep(args_st, abox, i as u32, &format!("{dest}.ap{i}"))
                    .unwrap();
                self.builder.build_store(gep, v).unwrap();
            }
            abox.into()
        };

        let thunk = self.fn_values[&format!("__tyra_spawn_thunk_{id}")]
            .as_global_value()
            .as_pointer_value();
        let spawn = self.module.get_function("tyra_task_spawn").unwrap();
        let h = self
            .builder
            .build_call(spawn, &[thunk.into(), args_box], &format!("{dest}.h"))
            .unwrap()
            .try_as_basic_value()
            .basic()
            .unwrap()
            .into_pointer_value();
        let handle = self.builder.build_ptr_to_int(h, i64t, dest).unwrap();
        self.values.insert(dest.to_string(), handle.into());
    }

    /// `dest = task.await` — `inttoptr` the handle, call `tyra_task_await`, and
    /// load the unboxed `result_type` from the returned box. `Unit` results have
    /// no runtime value, so `dest` binds the constant `i64 0` (SSA placeholder).
    pub(crate) fn emit_await(&mut self, dest: &str, task: &Operand, result_type: &Ty) {
        let ptr = self.ptr();
        let i64t = self.ctx.i64_type();
        let t = self.operand(task).into_int_value();
        let tptr = self.builder.build_int_to_ptr(t, ptr, &format!("{dest}.tptr")).unwrap();
        let await_fn = self.module.get_function("tyra_task_await").unwrap();
        let abox = self
            .builder
            .build_call(await_fn, &[tptr.into()], &format!("{dest}.box"))
            .unwrap()
            .try_as_basic_value()
            .basic()
            .unwrap()
            .into_pointer_value();

        if matches!(result_type, Ty::Unit | Ty::Never) {
            self.values.insert(dest.to_string(), i64t.const_zero().into());
            return;
        }
        let rty = self.ty_to_basic_type(result_type);
        let v = self.builder.build_load(rty, abox, dest).unwrap();
        self.values.insert(dest.to_string(), v);
    }

    /// `dest = tasks.join_all(list)` — await each `i64` handle in `list`, load
    /// the unboxed `T` from each result box, and build a fresh `List<T>`.
    pub(crate) fn emit_join_all(&mut self, dest: &str, list: &Operand, elem_type: &Ty) {
        let ptr = self.ptr();
        let i64t = self.ctx.i64_type();
        let (in_data, n) = self.list_data_len(list, dest);
        let elem_bt = self.ty_to_basic_type(elem_type);
        let esz = elem_bt.size_of().expect("join_all element type is sized");
        let tsz = self.builder.build_int_mul(n, esz, &format!("{dest}.tsz")).unwrap();
        let gc = self.module.get_function("GC_malloc").unwrap();
        let out_data = self
            .builder
            .build_call(gc, &[tsz.into()], &format!("{dest}.out"))
            .unwrap()
            .try_as_basic_value()
            .basic()
            .unwrap()
            .into_pointer_value();

        let ctr = self.builder.build_alloca(i64t, &format!("{dest}.ctr")).unwrap();
        self.builder.build_store(ctr, i64t.const_zero()).unwrap();

        let fv = self.builder.get_insert_block().unwrap().get_parent().unwrap();
        let loop_bb = self.ctx.append_basic_block(fv, &format!("{dest}.loop"));
        let body_bb = self.ctx.append_basic_block(fv, &format!("{dest}.body"));
        let end_bb = self.ctx.append_basic_block(fv, &format!("{dest}.end"));
        self.builder.build_unconditional_branch(loop_bb).unwrap();

        self.builder.position_at_end(loop_bb);
        let i = self.builder.build_load(i64t, ctr, &format!("{dest}.i")).unwrap().into_int_value();
        let done = self
            .builder
            .build_int_compare(IntPredicate::SGE, i, n, &format!("{dest}.done"))
            .unwrap();
        self.builder.build_conditional_branch(done, end_bb, body_bb).unwrap();

        self.builder.position_at_end(body_bb);
        let hgep = unsafe { self.builder.build_gep(i64t, in_data, &[i], &format!("{dest}.hgep")).unwrap() };
        let handle = self.builder.build_load(i64t, hgep, &format!("{dest}.handle")).unwrap().into_int_value();
        let tptr = self.builder.build_int_to_ptr(handle, ptr, &format!("{dest}.tptr")).unwrap();
        let await_fn = self.module.get_function("tyra_task_await").unwrap();
        let abox = self
            .builder
            .build_call(await_fn, &[tptr.into()], &format!("{dest}.box"))
            .unwrap()
            .try_as_basic_value()
            .basic()
            .unwrap()
            .into_pointer_value();
        let val = self.builder.build_load(elem_bt, abox, &format!("{dest}.val")).unwrap();
        let ogep = unsafe { self.builder.build_gep(elem_bt, out_data, &[i], &format!("{dest}.ogep")).unwrap() };
        self.builder.build_store(ogep, val).unwrap();
        self.incr_counter(ctr, i, dest);
        self.builder.build_unconditional_branch(loop_bb).unwrap();

        self.builder.position_at_end(end_bb);
        let list_mono = Ty::Generic("List".into(), vec![elem_type.clone()]).monomorphized_name();
        let list_ty = self.struct_types[&list_mono];
        let result = self.build_list_struct(list_ty, out_data.into(), n.into(), dest);
        self.values.insert(dest.to_string(), result);
    }

    /// `dest = tasks.select(list)` — hand the handle array straight to
    /// `tyra_task_select`, returning a new `Task<T>` handle (as `i64`). `T` is
    /// extracted later by the caller's `.await`, so `elem_type` is unused here
    /// (kept for round-trip fidelity with the MIR).
    pub(crate) fn emit_select(&mut self, dest: &str, list: &Operand, _elem_type: &Ty) {
        let i64t = self.ctx.i64_type();
        let (in_data, n) = self.list_data_len(list, dest);
        let select_fn = self.module.get_function("tyra_task_select").unwrap();
        let tptr = self
            .builder
            .build_call(select_fn, &[in_data.into(), n.into()], &format!("{dest}.tptr"))
            .unwrap()
            .try_as_basic_value()
            .basic()
            .unwrap()
            .into_pointer_value();
        let handle = self.builder.build_ptr_to_int(tptr, i64t, dest).unwrap();
        self.values.insert(dest.to_string(), handle.into());
    }

    /// Fill every pre-declared `@__tyra_spawn_thunk_N` body: unpack the argument
    /// struct, call the target function, and box the result (or return null for
    /// `Unit`). Runs after all user bodies so the target functions are defined.
    pub(crate) fn emit_spawn_thunk_bodies(&mut self) {
        // I6: thunks carry no subprogram, so their instructions must not inherit
        // the last user function's debug scope.
        self.clear_debug_line();
        let descs = self.spawn_thunks.clone();
        let ptr = self.ptr();
        for desc in &descs {
            let id = desc.id;
            let fv = self.fn_values[&format!("__tyra_spawn_thunk_{id}")];
            let entry = self.ctx.append_basic_block(fv, "entry");
            self.builder.position_at_end(entry);
            let args_param = fv.get_nth_param(0).unwrap().into_pointer_value();

            let mut call_args: Vec<BasicMetadataValueEnum<'ctx>> = Vec::with_capacity(desc.arg_types.len());
            if !desc.arg_types.is_empty() {
                let args_st = self.struct_types[&format!("__tyra_spawn_args_{id}")];
                for (i, ty) in desc.arg_types.iter().enumerate() {
                    let bt = self.ty_to_basic_type(ty);
                    let gep = self
                        .builder
                        .build_struct_gep(args_st, args_param, i as u32, &format!("a{i}.ptr"))
                        .unwrap();
                    let v = self.builder.build_load(bt, gep, &format!("a{i}")).unwrap();
                    call_args.push(v.into());
                }
            }

            let target = self.fn_values[&desc.func];
            if matches!(desc.result_type, Ty::Unit | Ty::Never) {
                self.builder.build_call(target, &call_args, "").unwrap();
                let null = ptr.const_null();
                self.builder.build_return(Some(&null)).unwrap();
            } else {
                let result = self
                    .builder
                    .build_call(target, &call_args, "result")
                    .unwrap()
                    .try_as_basic_value()
                    .basic()
                    .expect("spawn target with a non-Unit result must return a value");
                let rty = self.ty_to_basic_type(&desc.result_type);
                let size = rty.size_of().expect("spawn result type is sized");
                let gc = self.module.get_function("GC_malloc").unwrap();
                let rbox = self
                    .builder
                    .build_call(gc, &[size.into()], "box")
                    .unwrap()
                    .try_as_basic_value()
                    .basic()
                    .unwrap()
                    .into_pointer_value();
                self.builder.build_store(rbox, result).unwrap();
                self.builder.build_return(Some(&rbox)).unwrap();
            }
        }
    }
}
