//! Inkwell I4h: list higher-order builtins (`map`/`filter`/`fold`) driven by a
//! fat-pointer closure (ADR-0011).
//!
//! Each builtin takes a `List<T>` and a `__closure_fat { fn_ptr, env_ptr }` and
//! emits a counter loop that applies the closure per element via an indirect
//! call whose implicit first argument is `env_ptr` (the I4g calling convention):
//!
//! - `__list_map_{int,str}(xs, f) -> List<T>` — new list, same length; callback
//!   `T (ptr env, T elem)`.
//! - `__list_filter_{int,str}(xs, f) -> List<T>` — keep where the predicate
//!   holds; callback `i1 (ptr env, T elem)`; result length ≤ input.
//! - `__list_fold_{int,str}(xs, init, f) -> T` — left fold; callback
//!   `T (ptr env, T acc, T elem)`.
//!
//! Mirrors the legacy `emit_list_map`/`emit_list_filter`/`emit_list_fold`
//! (builtins.rs) EXACTLY in semantics (parity is the bar), with two deliberate
//! deviations shared with the rest of the inkwell migration: (1) the defensive
//! `GC_malloc` null-check + `abort` branch is dropped (Boehm aborts on OOM
//! internally; the branch would split the block and complicate phi bookkeeping),
//! and (2) value handles carry their own LLVM type, so element/list types come
//! from `ty_to_basic_type` rather than string tables.
//!
//! `Int` elements are `i64`; `String` (and any boxed/data element) are `ptr` —
//! both occupy 8 bytes, so the data buffer is `len * 8` exactly as legacy.

use inkwell::IntPredicate;
use inkwell::types::{BasicType, BasicTypeEnum, StructType};
use inkwell::values::{IntValue, PointerValue};

use tyra_mir::Operand;
use tyra_types::Ty;

use crate::inkwell_codegen::CodeGen;

const LIST_HIGHER: &[&str] = &[
    "__list_map_int",
    "__list_map_str",
    "__list_filter_int",
    "__list_filter_str",
    "__list_fold_int",
    "__list_fold_str",
];

impl<'ctx> CodeGen<'ctx> {
    /// Is `name` a list higher-order builtin (I4h)?
    pub(crate) fn is_list_higher_builtin(name: &str) -> bool {
        LIST_HIGHER.contains(&name)
    }

    /// Emit a list higher-order builtin. Returns `false` if `fname` is not in
    /// this slice (caller falls through to the next dispatch).
    pub(crate) fn emit_list_higher_builtin(
        &mut self,
        dest: &Option<String>,
        fname: &str,
        args: &[Operand],
    ) -> bool {
        let d = dest.as_deref();
        match fname {
            "__list_map_int" => self.emit_list_map(d, args, false),
            "__list_map_str" => self.emit_list_map(d, args, true),
            "__list_filter_int" => self.emit_list_filter(d, args, false),
            "__list_filter_str" => self.emit_list_filter(d, args, true),
            "__list_fold_int" => self.emit_list_fold(d, args, false),
            "__list_fold_str" => self.emit_list_fold(d, args, true),
            _ => return false,
        }
        true
    }

    /// `(elem_basic_type, List<elem> struct type)` for the int / str variants.
    fn list_higher_types(&self, is_str: bool) -> (BasicTypeEnum<'ctx>, StructType<'ctx>) {
        let elem = if is_str { Ty::String } else { Ty::Int };
        let mono = Ty::Generic("List".into(), vec![elem.clone()]).monomorphized_name();
        let list_ty = *self.struct_types.get(&mono).unwrap_or_else(|| {
            panic!("`{mono}` struct must be registered for list higher-order builtin")
        });
        (self.ty_to_basic_type(&elem), list_ty)
    }

    /// `__list_map_{int,str}(xs, f)` — apply the closure to each element into a
    /// fresh `len`-sized buffer; the result list keeps the input length.
    fn emit_list_map(&mut self, dest: Option<&str>, args: &[Operand], is_str: bool) {
        let d = dest.unwrap_or("_lmap");
        let i64t = self.ctx.i64_type();
        let (elem_bt, list_ty) = self.list_higher_types(is_str);
        let (data, len) = self.list_data_len(&args[0], d);

        let newdata = self.alloc_slots(len, d);
        let (fnp, envp) = self.load_closure_fields(&args[1], d);
        let ctr = self
            .builder
            .build_alloca(i64t, &format!("{d}.ctr"))
            .unwrap();
        self.builder.build_store(ctr, i64t.const_zero()).unwrap();

        let fv = self
            .builder
            .get_insert_block()
            .unwrap()
            .get_parent()
            .unwrap();
        let loop_bb = self.ctx.append_basic_block(fv, &format!("{d}.loop"));
        let body_bb = self.ctx.append_basic_block(fv, &format!("{d}.body"));
        let end_bb = self.ctx.append_basic_block(fv, &format!("{d}.end"));
        self.builder.build_unconditional_branch(loop_bb).unwrap();

        self.builder.position_at_end(loop_bb);
        let i = self
            .builder
            .build_load(i64t, ctr, &format!("{d}.i"))
            .unwrap()
            .into_int_value();
        let done = self
            .builder
            .build_int_compare(IntPredicate::SGE, i, len, &format!("{d}.done"))
            .unwrap();
        self.builder
            .build_conditional_branch(done, end_bb, body_bb)
            .unwrap();

        self.builder.position_at_end(body_bb);
        let srcp = unsafe {
            self.builder
                .build_gep(elem_bt, data, &[i], &format!("{d}.srcp"))
                .unwrap()
        };
        let elem = self
            .builder
            .build_load(elem_bt, srcp, &format!("{d}.elem"))
            .unwrap();
        // Callback signature: T (ptr env, T elem).
        let fn_ty = elem_bt.fn_type(&[self.ptr().into(), elem_bt.into()], false);
        let mapped = self
            .builder
            .build_indirect_call(
                fn_ty,
                fnp,
                &[envp.into(), elem.into()],
                &format!("{d}.mapped"),
            )
            .unwrap()
            .try_as_basic_value()
            .basic()
            .unwrap();
        let dstp = unsafe {
            self.builder
                .build_gep(elem_bt, newdata, &[i], &format!("{d}.dstp"))
                .unwrap()
        };
        self.builder.build_store(dstp, mapped).unwrap();
        self.incr_counter(ctr, i, d);
        self.builder.build_unconditional_branch(loop_bb).unwrap();

        self.builder.position_at_end(end_bb);
        let result = self.build_list_struct(list_ty, newdata.into(), len.into(), d);
        self.values.insert(d.to_string(), result);
    }

    /// `__list_filter_{int,str}(xs, f)` — keep elements for which the predicate
    /// returns true, compacted into a `len`-sized buffer; the result length is
    /// the count kept.
    fn emit_list_filter(&mut self, dest: Option<&str>, args: &[Operand], is_str: bool) {
        let d = dest.unwrap_or("_lfilt");
        let i64t = self.ctx.i64_type();
        let i1t = self.ctx.bool_type();
        let (elem_bt, list_ty) = self.list_higher_types(is_str);
        let (data, len) = self.list_data_len(&args[0], d);

        let outdata = self.alloc_slots(len, d);
        let (fnp, envp) = self.load_closure_fields(&args[1], d);
        let ctr = self
            .builder
            .build_alloca(i64t, &format!("{d}.ctr"))
            .unwrap();
        self.builder.build_store(ctr, i64t.const_zero()).unwrap();
        let outctr = self
            .builder
            .build_alloca(i64t, &format!("{d}.outctr"))
            .unwrap();
        self.builder.build_store(outctr, i64t.const_zero()).unwrap();

        let fv = self
            .builder
            .get_insert_block()
            .unwrap()
            .get_parent()
            .unwrap();
        let loop_bb = self.ctx.append_basic_block(fv, &format!("{d}.loop"));
        let body_bb = self.ctx.append_basic_block(fv, &format!("{d}.body"));
        let keep_bb = self.ctx.append_basic_block(fv, &format!("{d}.keep"));
        let skip_bb = self.ctx.append_basic_block(fv, &format!("{d}.skip"));
        let end_bb = self.ctx.append_basic_block(fv, &format!("{d}.end"));
        self.builder.build_unconditional_branch(loop_bb).unwrap();

        self.builder.position_at_end(loop_bb);
        let i = self
            .builder
            .build_load(i64t, ctr, &format!("{d}.i"))
            .unwrap()
            .into_int_value();
        let done = self
            .builder
            .build_int_compare(IntPredicate::SGE, i, len, &format!("{d}.done"))
            .unwrap();
        self.builder
            .build_conditional_branch(done, end_bb, body_bb)
            .unwrap();

        self.builder.position_at_end(body_bb);
        let srcp = unsafe {
            self.builder
                .build_gep(elem_bt, data, &[i], &format!("{d}.srcp"))
                .unwrap()
        };
        let elem = self
            .builder
            .build_load(elem_bt, srcp, &format!("{d}.elem"))
            .unwrap();
        // Predicate signature: i1 (ptr env, T elem).
        let fn_ty = i1t.fn_type(&[self.ptr().into(), elem_bt.into()], false);
        let raw = self
            .builder
            .build_indirect_call(fn_ty, fnp, &[envp.into(), elem.into()], &format!("{d}.raw"))
            .unwrap()
            .try_as_basic_value()
            .basic()
            .unwrap()
            .into_int_value();
        self.builder
            .build_conditional_branch(raw, keep_bb, skip_bb)
            .unwrap();

        self.builder.position_at_end(keep_bb);
        let oi = self
            .builder
            .build_load(i64t, outctr, &format!("{d}.oi"))
            .unwrap()
            .into_int_value();
        let dstp = unsafe {
            self.builder
                .build_gep(elem_bt, outdata, &[oi], &format!("{d}.dstp"))
                .unwrap()
        };
        self.builder.build_store(dstp, elem).unwrap();
        self.incr_counter(outctr, oi, &format!("{d}.o"));
        self.builder.build_unconditional_branch(skip_bb).unwrap();

        self.builder.position_at_end(skip_bb);
        self.incr_counter(ctr, i, d);
        self.builder.build_unconditional_branch(loop_bb).unwrap();

        self.builder.position_at_end(end_bb);
        let outlen = self
            .builder
            .build_load(i64t, outctr, &format!("{d}.outlen"))
            .unwrap();
        let result = self.build_list_struct(list_ty, outdata.into(), outlen, d);
        self.values.insert(d.to_string(), result);
    }

    /// `__list_fold_{int,str}(xs, init, f)` — left fold; the accumulator and each
    /// element share the element type.
    fn emit_list_fold(&mut self, dest: Option<&str>, args: &[Operand], is_str: bool) {
        let d = dest.unwrap_or("_lfold");
        let i64t = self.ctx.i64_type();
        let (elem_bt, _) = self.list_higher_types(is_str);
        let (data, len) = self.list_data_len(&args[0], d);
        let init = self.operand(&args[1]);

        let acc = self
            .builder
            .build_alloca(elem_bt, &format!("{d}.acc"))
            .unwrap();
        self.builder.build_store(acc, init).unwrap();
        let (fnp, envp) = self.load_closure_fields(&args[2], d);
        let ctr = self
            .builder
            .build_alloca(i64t, &format!("{d}.ctr"))
            .unwrap();
        self.builder.build_store(ctr, i64t.const_zero()).unwrap();

        let fv = self
            .builder
            .get_insert_block()
            .unwrap()
            .get_parent()
            .unwrap();
        let loop_bb = self.ctx.append_basic_block(fv, &format!("{d}.loop"));
        let body_bb = self.ctx.append_basic_block(fv, &format!("{d}.body"));
        let end_bb = self.ctx.append_basic_block(fv, &format!("{d}.end"));
        self.builder.build_unconditional_branch(loop_bb).unwrap();

        self.builder.position_at_end(loop_bb);
        let i = self
            .builder
            .build_load(i64t, ctr, &format!("{d}.i"))
            .unwrap()
            .into_int_value();
        let done = self
            .builder
            .build_int_compare(IntPredicate::SGE, i, len, &format!("{d}.done"))
            .unwrap();
        self.builder
            .build_conditional_branch(done, end_bb, body_bb)
            .unwrap();

        self.builder.position_at_end(body_bb);
        let srcp = unsafe {
            self.builder
                .build_gep(elem_bt, data, &[i], &format!("{d}.srcp"))
                .unwrap()
        };
        let elem = self
            .builder
            .build_load(elem_bt, srcp, &format!("{d}.elem"))
            .unwrap();
        let cur = self
            .builder
            .build_load(elem_bt, acc, &format!("{d}.cur"))
            .unwrap();
        // Callback signature: T (ptr env, T acc, T elem).
        let fn_ty = elem_bt.fn_type(&[self.ptr().into(), elem_bt.into(), elem_bt.into()], false);
        let new = self
            .builder
            .build_indirect_call(
                fn_ty,
                fnp,
                &[envp.into(), cur.into(), elem.into()],
                &format!("{d}.new"),
            )
            .unwrap()
            .try_as_basic_value()
            .basic()
            .unwrap();
        self.builder.build_store(acc, new).unwrap();
        self.incr_counter(ctr, i, d);
        self.builder.build_unconditional_branch(loop_bb).unwrap();

        self.builder.position_at_end(end_bb);
        let result = self.builder.build_load(elem_bt, acc, d).unwrap();
        self.values.insert(d.to_string(), result);
    }

    /// GC-allocate `len * 8` bytes for an output element buffer (8 bytes per
    /// slot — both `i64` and `ptr` elements are 8-wide, matching legacy). The
    /// Boehm OOM branch is intentionally omitted (see the module note).
    fn alloc_slots(&mut self, len: IntValue<'ctx>, d: &str) -> PointerValue<'ctx> {
        let i64t = self.ctx.i64_type();
        let size = self
            .builder
            .build_int_mul(len, i64t.const_int(8, false), &format!("{d}.size"))
            .unwrap();
        let gc = self.module.get_function("GC_malloc").unwrap();
        self.builder
            .build_call(gc, &[size.into()], &format!("{d}.buf"))
            .unwrap()
            .try_as_basic_value()
            .basic()
            .unwrap()
            .into_pointer_value()
    }
}
