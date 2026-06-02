//! Inkwell I4b (slice A): scalar `__list_int_*` builtins.
//!
//! These six builtins operate on a `List<Int>` (`{ ptr, i64 }`, §11) with no
//! closure callback, mirroring the legacy text backend's `emit_list_int_*`
//! helpers EXACTLY (the migration bar is execution parity, not IR identity):
//!
//! - `__list_int_push(l, x) -> List<Int>` — immutable append. Identical to the
//!   `ListPush` instruction (I3), so it delegates to `emit_list_push`.
//! - `__list_int_sum(l) -> Int` — fold `+`, identity 0.
//! - `__list_int_contains(l, x) -> Bool` — linear search.
//! - `__list_int_index_of(l, x) -> Option<Int>` — first-match index.
//! - `__list_int_max(l)` / `__list_int_min(l) -> Option<Int>` — extremum, `None`
//!   for an empty list.
//!
//! The four search/aggregate forms genuinely branch (a counter loop), so they
//! split the current block. `emit_function_body`'s per-instruction `pred_blocks`
//! sync keeps deferred phi resolution pointing at the loop's *exit* block (same
//! precedent as I3 `ListGetSafe`). The loop counter / accumulator / result use
//! `alloca` + load/store rather than phi, faithful to the legacy lowering.

use inkwell::IntPredicate;
use inkwell::types::StructType;
use inkwell::values::{AggregateValueEnum, BasicValueEnum, IntValue, PointerValue};

use tyra_mir::Operand;
use tyra_types::Ty;

use crate::inkwell_codegen::CodeGen;

const LIST_INT: &[&str] = &[
    "__list_int_push",
    "__list_int_sum",
    "__list_int_contains",
    "__list_int_index_of",
    "__list_int_max",
    "__list_int_min",
];

impl<'ctx> CodeGen<'ctx> {
    /// Is `name` a scalar `__list_int_*` builtin (I4b slice A)?
    pub(crate) fn is_list_int_builtin(name: &str) -> bool {
        LIST_INT.contains(&name)
    }

    /// Emit a scalar `__list_int_*` builtin. Returns `false` if `fname` is not
    /// in this slice (caller falls through to the next dispatch).
    pub(crate) fn emit_list_int_builtin(
        &mut self,
        dest: &Option<String>,
        fname: &str,
        args: &[Operand],
    ) -> bool {
        let d = dest.as_deref();
        match fname {
            "__list_int_push" => {
                // Identical semantics to the `ListPush` instruction: immutable
                // append of an Int. Reuse the I3 emitter (memcpy prefix, no
                // block split).
                let dn = d.unwrap_or("_list_push").to_string();
                self.emit_list_push(&dn, &args[0], &args[1], &Ty::Int);
            }
            "__list_int_sum" => self.emit_list_int_sum(d, args),
            "__list_int_contains" => self.emit_list_int_contains(d, args),
            "__list_int_index_of" => self.emit_list_int_index_of(d, args),
            "__list_int_max" => self.emit_list_int_min_max(d, args, true),
            "__list_int_min" => self.emit_list_int_min_max(d, args, false),
            _ => return false,
        }
        true
    }

    /// `Option<Int>` struct type (`{ i8, i64 }`), registered by monomorphization.
    fn option_int_ty(&self) -> StructType<'ctx> {
        let mono = Ty::Generic("Option".into(), vec![Ty::Int]).monomorphized_name();
        *self
            .struct_types
            .get(&mono)
            .unwrap_or_else(|| panic!("`{mono}` struct must be registered for list_int index/min/max"))
    }

    /// Extract `(data, len)` from a list operand handle. Layout-agnostic — the
    /// physical struct is `{ ptr data, i64 len }` for every element type, so the
    /// I4h `List<String>` higher-order builtins reuse it directly.
    pub(crate) fn list_data_len(&mut self, list: &Operand, d: &str) -> (PointerValue<'ctx>, IntValue<'ctx>) {
        let lv = self.operand(list).into_struct_value();
        let data = self
            .builder
            .build_extract_value(lv, 0, &format!("{d}.data"))
            .unwrap()
            .into_pointer_value();
        let len = self
            .builder
            .build_extract_value(lv, 1, &format!("{d}.len"))
            .unwrap()
            .into_int_value();
        (data, len)
    }

    /// `__list_int_sum(list)` — accumulate `+` over the elements, identity 0.
    fn emit_list_int_sum(&mut self, dest: Option<&str>, args: &[Operand]) {
        let d = dest.unwrap_or("_list_sum");
        let i64t = self.ctx.i64_type();
        let (data, len) = self.list_data_len(&args[0], d);

        let acc = self.builder.build_alloca(i64t, &format!("{d}.acc")).unwrap();
        self.builder.build_store(acc, i64t.const_zero()).unwrap();
        let ctr = self.builder.build_alloca(i64t, &format!("{d}.ctr")).unwrap();
        self.builder.build_store(ctr, i64t.const_zero()).unwrap();

        let fv = self.builder.get_insert_block().unwrap().get_parent().unwrap();
        let loop_bb = self.ctx.append_basic_block(fv, &format!("{d}.loop"));
        let body_bb = self.ctx.append_basic_block(fv, &format!("{d}.body"));
        let end_bb = self.ctx.append_basic_block(fv, &format!("{d}.end"));
        self.builder.build_unconditional_branch(loop_bb).unwrap();

        self.builder.position_at_end(loop_bb);
        let i = self.builder.build_load(i64t, ctr, &format!("{d}.i")).unwrap().into_int_value();
        let done = self
            .builder
            .build_int_compare(IntPredicate::SGE, i, len, &format!("{d}.done"))
            .unwrap();
        self.builder.build_conditional_branch(done, end_bb, body_bb).unwrap();

        self.builder.position_at_end(body_bb);
        let p = unsafe { self.builder.build_gep(i64t, data, &[i], &format!("{d}.p")).unwrap() };
        let v = self.builder.build_load(i64t, p, &format!("{d}.v")).unwrap().into_int_value();
        let cur = self.builder.build_load(i64t, acc, &format!("{d}.cur")).unwrap().into_int_value();
        let sum = self.builder.build_int_add(cur, v, &format!("{d}.sum")).unwrap();
        self.builder.build_store(acc, sum).unwrap();
        self.incr_counter(ctr, i, d);
        self.builder.build_unconditional_branch(loop_bb).unwrap();

        self.builder.position_at_end(end_bb);
        let result = self.builder.build_load(i64t, acc, d).unwrap();
        self.values.insert(d.to_string(), result);
    }

    /// `__list_int_contains(list, x)` — linear search, returns `Bool` (i1).
    fn emit_list_int_contains(&mut self, dest: Option<&str>, args: &[Operand]) {
        let d = dest.unwrap_or("_list_contains");
        let i64t = self.ctx.i64_type();
        let i1t = self.ctx.bool_type();
        let (data, len) = self.list_data_len(&args[0], d);
        let x = self.operand(&args[1]).into_int_value();

        let res = self.builder.build_alloca(i1t, &format!("{d}.res")).unwrap();
        self.builder.build_store(res, i1t.const_zero()).unwrap();
        let ctr = self.builder.build_alloca(i64t, &format!("{d}.ctr")).unwrap();
        self.builder.build_store(ctr, i64t.const_zero()).unwrap();

        let fv = self.builder.get_insert_block().unwrap().get_parent().unwrap();
        let loop_bb = self.ctx.append_basic_block(fv, &format!("{d}.loop"));
        let body_bb = self.ctx.append_basic_block(fv, &format!("{d}.body"));
        let hit_bb = self.ctx.append_basic_block(fv, &format!("{d}.hit"));
        let cont_bb = self.ctx.append_basic_block(fv, &format!("{d}.cont"));
        let end_bb = self.ctx.append_basic_block(fv, &format!("{d}.end"));
        self.builder.build_unconditional_branch(loop_bb).unwrap();

        self.builder.position_at_end(loop_bb);
        let i = self.builder.build_load(i64t, ctr, &format!("{d}.i")).unwrap().into_int_value();
        let done = self
            .builder
            .build_int_compare(IntPredicate::SGE, i, len, &format!("{d}.done"))
            .unwrap();
        self.builder.build_conditional_branch(done, end_bb, body_bb).unwrap();

        self.builder.position_at_end(body_bb);
        let p = unsafe { self.builder.build_gep(i64t, data, &[i], &format!("{d}.p")).unwrap() };
        let v = self.builder.build_load(i64t, p, &format!("{d}.v")).unwrap().into_int_value();
        let eq = self.builder.build_int_compare(IntPredicate::EQ, v, x, &format!("{d}.eq")).unwrap();
        self.builder.build_conditional_branch(eq, hit_bb, cont_bb).unwrap();

        self.builder.position_at_end(hit_bb);
        self.builder.build_store(res, i1t.const_int(1, false)).unwrap();
        self.builder.build_unconditional_branch(end_bb).unwrap();

        self.builder.position_at_end(cont_bb);
        self.incr_counter(ctr, i, d);
        self.builder.build_unconditional_branch(loop_bb).unwrap();

        self.builder.position_at_end(end_bb);
        let result = self.builder.build_load(i1t, res, d).unwrap();
        self.values.insert(d.to_string(), result);
    }

    /// `__list_int_index_of(list, x)` — lowest matching index as `Some(i)`, else
    /// `None`.
    fn emit_list_int_index_of(&mut self, dest: Option<&str>, args: &[Operand]) {
        let d = dest.unwrap_or("_list_index_of");
        let i64t = self.ctx.i64_type();
        let opt_ty = self.option_int_ty();
        let (data, len) = self.list_data_len(&args[0], d);
        let x = self.operand(&args[1]).into_int_value();

        let slot = self.builder.build_alloca(opt_ty, &format!("{d}.slot")).unwrap();
        let none = self.build_option_int(opt_ty, 1, i64t.const_zero().into(), &format!("{d}.none"));
        self.builder.build_store(slot, none).unwrap();
        let ctr = self.builder.build_alloca(i64t, &format!("{d}.ctr")).unwrap();
        self.builder.build_store(ctr, i64t.const_zero()).unwrap();

        let fv = self.builder.get_insert_block().unwrap().get_parent().unwrap();
        let loop_bb = self.ctx.append_basic_block(fv, &format!("{d}.loop"));
        let body_bb = self.ctx.append_basic_block(fv, &format!("{d}.body"));
        let hit_bb = self.ctx.append_basic_block(fv, &format!("{d}.hit"));
        let cont_bb = self.ctx.append_basic_block(fv, &format!("{d}.cont"));
        let end_bb = self.ctx.append_basic_block(fv, &format!("{d}.end"));
        self.builder.build_unconditional_branch(loop_bb).unwrap();

        self.builder.position_at_end(loop_bb);
        let i = self.builder.build_load(i64t, ctr, &format!("{d}.i")).unwrap().into_int_value();
        let done = self
            .builder
            .build_int_compare(IntPredicate::SGE, i, len, &format!("{d}.done"))
            .unwrap();
        self.builder.build_conditional_branch(done, end_bb, body_bb).unwrap();

        self.builder.position_at_end(body_bb);
        let p = unsafe { self.builder.build_gep(i64t, data, &[i], &format!("{d}.p")).unwrap() };
        let v = self.builder.build_load(i64t, p, &format!("{d}.v")).unwrap().into_int_value();
        let eq = self.builder.build_int_compare(IntPredicate::EQ, v, x, &format!("{d}.eq")).unwrap();
        self.builder.build_conditional_branch(eq, hit_bb, cont_bb).unwrap();

        self.builder.position_at_end(hit_bb);
        let some = self.build_option_int(opt_ty, 0, i.into(), &format!("{d}.some"));
        self.builder.build_store(slot, some).unwrap();
        self.builder.build_unconditional_branch(end_bb).unwrap();

        self.builder.position_at_end(cont_bb);
        self.incr_counter(ctr, i, d);
        self.builder.build_unconditional_branch(loop_bb).unwrap();

        self.builder.position_at_end(end_bb);
        let result = self.builder.build_load(opt_ty, slot, d).unwrap();
        self.values.insert(d.to_string(), result);
    }

    /// `__list_int_max(list)` / `__list_int_min(list)` — extremum as `Some(v)`,
    /// `None` for an empty list. `is_max` selects `sgt` (else `slt`).
    fn emit_list_int_min_max(&mut self, dest: Option<&str>, args: &[Operand], is_max: bool) {
        let d = dest.unwrap_or(if is_max { "_list_max" } else { "_list_min" });
        let i64t = self.ctx.i64_type();
        let opt_ty = self.option_int_ty();
        let pred = if is_max { IntPredicate::SGT } else { IntPredicate::SLT };
        let (data, len) = self.list_data_len(&args[0], d);

        let slot = self.builder.build_alloca(opt_ty, &format!("{d}.slot")).unwrap();
        let empty = self
            .builder
            .build_int_compare(IntPredicate::EQ, len, i64t.const_zero(), &format!("{d}.empty"))
            .unwrap();

        let fv = self.builder.get_insert_block().unwrap().get_parent().unwrap();
        let none_bb = self.ctx.append_basic_block(fv, &format!("{d}.none"));
        let init_bb = self.ctx.append_basic_block(fv, &format!("{d}.init"));
        let loop_bb = self.ctx.append_basic_block(fv, &format!("{d}.loop"));
        let body_bb = self.ctx.append_basic_block(fv, &format!("{d}.body"));
        let upd_bb = self.ctx.append_basic_block(fv, &format!("{d}.upd"));
        let cont_bb = self.ctx.append_basic_block(fv, &format!("{d}.cont"));
        let some_bb = self.ctx.append_basic_block(fv, &format!("{d}.some"));
        let end_bb = self.ctx.append_basic_block(fv, &format!("{d}.end"));
        self.builder.build_conditional_branch(empty, none_bb, init_bb).unwrap();

        // Empty: store None.
        self.builder.position_at_end(none_bb);
        let none = self.build_option_int(opt_ty, 1, i64t.const_zero().into(), &format!("{d}.none"));
        self.builder.build_store(slot, none).unwrap();
        self.builder.build_unconditional_branch(end_bb).unwrap();

        // Init: best = data[0], ctr = 1.
        self.builder.position_at_end(init_bb);
        let p0 = unsafe {
            self.builder
                .build_gep(i64t, data, &[i64t.const_zero()], &format!("{d}.p0"))
                .unwrap()
        };
        let v0 = self.builder.build_load(i64t, p0, &format!("{d}.v0")).unwrap();
        let best = self.builder.build_alloca(i64t, &format!("{d}.best")).unwrap();
        self.builder.build_store(best, v0).unwrap();
        let ctr = self.builder.build_alloca(i64t, &format!("{d}.ctr")).unwrap();
        self.builder.build_store(ctr, i64t.const_int(1, false)).unwrap();
        self.builder.build_unconditional_branch(loop_bb).unwrap();

        self.builder.position_at_end(loop_bb);
        let i = self.builder.build_load(i64t, ctr, &format!("{d}.i")).unwrap().into_int_value();
        let done = self
            .builder
            .build_int_compare(IntPredicate::SGE, i, len, &format!("{d}.done"))
            .unwrap();
        self.builder.build_conditional_branch(done, some_bb, body_bb).unwrap();

        self.builder.position_at_end(body_bb);
        let p = unsafe { self.builder.build_gep(i64t, data, &[i], &format!("{d}.p")).unwrap() };
        let v = self.builder.build_load(i64t, p, &format!("{d}.v")).unwrap().into_int_value();
        let cur = self.builder.build_load(i64t, best, &format!("{d}.cur")).unwrap().into_int_value();
        let better = self.builder.build_int_compare(pred, v, cur, &format!("{d}.better")).unwrap();
        self.builder.build_conditional_branch(better, upd_bb, cont_bb).unwrap();

        self.builder.position_at_end(upd_bb);
        self.builder.build_store(best, v).unwrap();
        self.builder.build_unconditional_branch(cont_bb).unwrap();

        self.builder.position_at_end(cont_bb);
        self.incr_counter(ctr, i, d);
        self.builder.build_unconditional_branch(loop_bb).unwrap();

        // Loop done: store Some(best).
        self.builder.position_at_end(some_bb);
        let final_v = self.builder.build_load(i64t, best, &format!("{d}.final")).unwrap();
        let some = self.build_option_int(opt_ty, 0, final_v, &format!("{d}.some"));
        self.builder.build_store(slot, some).unwrap();
        self.builder.build_unconditional_branch(end_bb).unwrap();

        self.builder.position_at_end(end_bb);
        let result = self.builder.build_load(opt_ty, slot, d).unwrap();
        self.values.insert(d.to_string(), result);
    }

    /// `ctr = i + 1` store; shared loop-counter increment.
    pub(crate) fn incr_counter(&self, ctr: PointerValue<'ctx>, i: IntValue<'ctx>, d: &str) {
        let i64t = self.ctx.i64_type();
        let next = self.builder.build_int_add(i, i64t.const_int(1, false), &format!("{d}.next")).unwrap();
        self.builder.build_store(ctr, next).unwrap();
    }

    /// Build an `Option<Int>` struct value `{ i8 tag, i64 value }`.
    fn build_option_int(
        &self,
        opt_ty: StructType<'ctx>,
        tag: u64,
        value: BasicValueEnum<'ctx>,
        name: &str,
    ) -> BasicValueEnum<'ctx> {
        let i8t = self.ctx.i8_type();
        let mut agg: AggregateValueEnum = opt_ty.get_undef().into();
        agg = self
            .builder
            .build_insert_value(agg, i8t.const_int(tag, false), 0, &format!("{name}.s0"))
            .unwrap();
        agg = self
            .builder
            .build_insert_value(agg, value, 1, &format!("{name}.s1"))
            .unwrap();
        agg.into_struct_value().into()
    }
}
