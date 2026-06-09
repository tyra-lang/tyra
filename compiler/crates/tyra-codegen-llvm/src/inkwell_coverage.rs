//! Inkwell I5: coverage instrumentation emission (ADR-0014).
//!
//! The pure data side — building the counter map, serializing the covmap
//! sidecar, merging `.covraw` files, formatting reports — lives in
//! `crate::coverage` and is shared with the legacy backend. This module ports
//! only the IR-emitting half to the inkwell builder:
//!
//! - `declare_coverage`: the `@.tyra_counters = [N x i64] zeroinitializer`
//!   global and the `tyra_cov_init(ptr, i64)` extern.
//! - `emit_cov_increment`: a per-basic-block `atomicrmw add … monotonic` on the
//!   counter for the block's `(file_id, line)` — the legacy text emitted this
//!   as a literal `atomicrmw` string; here it is `build_atomicrmw`.
//! - `emit_cov_init_call`: the `tyra_cov_init(@.tyra_counters, N)` call woven
//!   into `main` after the runtime is initialized.
//!
//! All three are no-ops when `cov_map` is `None` (ordinary builds).

use inkwell::values::PointerValue;
use inkwell::{AtomicOrdering, AtomicRMWBinOp};

use tyra_mir::SourceLoc;

use crate::inkwell_codegen::CodeGen;

/// The counter-array global name (shared with the runtime flusher).
const COUNTERS: &str = ".tyra_counters";

impl<'ctx> CodeGen<'ctx> {
    /// Effective counter-array length (`n`, but at least 1 so the `[N x i64]`
    /// type is well-formed when a program has no instrumentable locations).
    fn cov_len(&self) -> Option<u32> {
        self.cov_map
            .as_ref()
            .map(|cm| if cm.n == 0 { 1 } else { cm.n })
    }

    /// Declare the `.tyra_counters` global (`[N x i64] zeroinitializer`) and the
    /// `tyra_cov_init` extern. No-op without a `cov_map`.
    pub(crate) fn declare_coverage(&mut self) {
        let Some(n) = self.cov_len() else { return };
        let i64t = self.ctx.i64_type();
        let arr_ty = i64t.array_type(n);
        let g = self.module.add_global(arr_ty, None, COUNTERS);
        g.set_initializer(&arr_ty.const_zero());

        // `declare void @tyra_cov_init(ptr, i64)`.
        let void = self.ctx.void_type();
        let ptr = self.ptr();
        let fn_ty = void.fn_type(&[ptr.into(), i64t.into()], false);
        self.module.add_function("tyra_cov_init", fn_ty, None);
    }

    /// Emit the counter increment for `loc`'s basic block: GEP into the counter
    /// array and `atomicrmw add … monotonic`. No-op for a dummy loc, without a
    /// `cov_map`, or for a loc with no assigned counter.
    pub(crate) fn emit_cov_increment(&mut self, loc: SourceLoc) {
        if loc.is_dummy() {
            return;
        }
        // Resolve (idx, n) and drop the cov_map borrow before touching builder.
        let (idx, n) = match &self.cov_map {
            Some(cm) => match cm.counter_for.get(&(loc.file_id, loc.line)) {
                Some(&idx) => (idx, if cm.n == 0 { 1 } else { cm.n }),
                None => return,
            },
            None => return,
        };
        let i64t = self.ctx.i64_type();
        let arr_ty = i64t.array_type(n);
        let global = self.module.get_global(COUNTERS).unwrap().as_pointer_value();
        let zero = i64t.const_zero();
        let idxv = i64t.const_int(idx as u64, false);
        let gep = unsafe {
            self.builder
                .build_in_bounds_gep(arr_ty, global, &[zero, idxv], "__cov_gep")
                .unwrap()
        };
        self.builder
            .build_atomicrmw(
                AtomicRMWBinOp::Add,
                gep,
                i64t.const_int(1, false),
                AtomicOrdering::Monotonic,
            )
            .unwrap();
    }

    /// Increment the entry-block counter for `f` (its first non-dummy loc), so
    /// the function is recorded as entered. Mirrors the legacy entry increment.
    pub(crate) fn emit_cov_entry(&mut self, f: &tyra_mir::Function) {
        if self.cov_map.is_none() {
            return;
        }
        if let Some(loc) = f.body.iter().find(|s| !s.loc.is_dummy()).map(|s| s.loc) {
            self.emit_cov_increment(loc);
        }
    }

    /// Emit `tyra_cov_init(@.tyra_counters, N)` (woven into `main` after runtime
    /// init). No-op without a `cov_map`.
    pub(crate) fn emit_cov_init_call(&mut self) {
        let Some(n) = self.cov_len() else { return };
        let i64t = self.ctx.i64_type();
        let counters: PointerValue<'ctx> =
            self.module.get_global(COUNTERS).unwrap().as_pointer_value();
        let init = self.module.get_function("tyra_cov_init").unwrap();
        self.builder
            .build_call(
                init,
                &[counters.into(), i64t.const_int(n as u64, false).into()],
                "",
            )
            .unwrap();
    }
}
