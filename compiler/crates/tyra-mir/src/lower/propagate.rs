// Propagate expression lowering (? operator).
//
// Handles ExprKind::Propagate for Option<T> and Result<T, E>:
// extracts value on success, early-returns on failure with optional
// Into<F> error conversion (spec §12.2).

use tyra_ast::*;
use tyra_types::Ty;

use crate::ir::*;

impl super::LowerCtx {
    /// Lower `ExprKind::Propagate(inner)` — the `?` operator.
    ///
    /// Extracts the success payload on the happy path and emits an
    /// early return (with optional `Into<F>` error conversion) on
    /// the failure path.
    pub(super) fn lower_propagate(
        &mut self,
        inner: &Expr,
        body: &mut Vec<Instruction>,
    ) -> String {
        // ? operator: extract value on success, early-return on failure
        let inner_val = self.lower_expr(inner, body);

        // Determine the ADT type of the inner expression
        let inner_type = self
            .generic_var_types
            .get(&inner_val)
            .cloned()
            .unwrap_or(self.current_fn_return_type.clone());
        let type_name = inner_type.monomorphized_name();

        // Extract tag
        let tag = self.fresh_temp();
        body.push(Instruction::AdtTag {
            dest: tag.clone(),
            obj: Operand::Var(inner_val.clone()),
            type_name: type_name.clone(),
        });

        // Check if failure (tag != 0 means None/Err)
        let zero = self.fresh_temp();
        body.push(Instruction::Const {
            dest: zero.clone(),
            value: Constant::Int(0),
        });
        let is_ok = self.fresh_temp();
        body.push(Instruction::BinOp {
            dest: is_ok.clone(),
            op: MirBinOp::EqInt,
            lhs: Operand::Var(tag),
            rhs: Operand::Var(zero),
        });

        let ok_label = self.fresh_label("propagate_ok");
        let fail_label = self.fresh_label("propagate_fail");

        body.push(Instruction::BranchIf {
            cond: Operand::Var(is_ok),
            true_label: ok_label.clone(),
            false_label: fail_label.clone(),
        });

        // Failure path: return None/Err from current function
        body.push(Instruction::Label(fail_label));
        if inner_type.is_result() {
            // For Result: extract err_value and re-wrap as Err.
            // spec §12.2: If inner error type E != enclosing error type F,
            // convert via Into<F>: `return Err(e.into())`.
            let ret_type = &self.current_fn_return_type.clone();
            self.register_adt_type(ret_type);
            let ret_type_name = ret_type.monomorphized_name();
            let err_val = self.fresh_temp();
            body.push(Instruction::AdtPayload {
                dest: err_val.clone(),
                obj: Operand::Var(inner_val.clone()),
                type_name: type_name.clone(),
                field_index: 2, // err_value field for Result
            });

            // Apply Into<F> conversion if error types differ (spec §12.2)
            let final_err_val =
                if let (Some(inner_err), Some(ret_err)) =
                    (inner_type.result_err_type(), ret_type.result_err_type())
                {
                    if inner_err != ret_err {
                        let inner_err_name = inner_err.monomorphized_name();
                        let into_key =
                            (inner_err_name.clone(), "into".to_string());
                        if let Some(mangled) = self.impl_methods.get(&into_key).cloned() {
                            // Call E__into(err_val) to convert error type
                            let converted = self.fresh_temp();
                            body.push(Instruction::Call {
                                dest: Some(converted.clone()),
                                func: mangled,
                                args: vec![Operand::Var(err_val.clone())],
                            });
                            converted
                        } else {
                            // §12.2 E0311 enforces an Into<F> impl for
                            // fully-resolved concrete types before lowering.
                            // This branch only fires for generic / pre-
                            // monomorphized edge cases where the `into`
                            // method is not yet registered in impl_methods.
                            // Silent identity is safe when the runtime
                            // representations of the two error ADTs happen
                            // to coincide (same struct layout); otherwise
                            // it would emit miscompiled code. We flag it
                            // in debug builds so integration tests catch
                            // any new regressions, and fall through as
                            // identity in release.
                            // TODO: add a MIR test exercising ? on a
                            // generic Result to lock this path down.
                            debug_assert!(
                                false,
                                "MIR lowering: missing Into<{}> for {} — \
                                 type checker should have caught this (§12.2 E0311)",
                                ret_err.display_name(),
                                inner_err.display_name(),
                            );
                            err_val.clone()
                        }
                    } else {
                        // Into<T> for T: identity, no conversion needed
                        err_val.clone()
                    }
                } else {
                    err_val.clone()
                };

            let ret_err = self.fresh_temp();
            body.push(Instruction::AdtInit {
                dest: ret_err.clone(),
                type_name: ret_type_name,
                tag: 1,
                fields: vec![
                    Operand::Const(Constant::Int(0)),
                    Operand::Var(final_err_val),
                ],
            });
            // spec §12.3: emit deferred expressions before early return
            self.emit_deferred(body);
            body.push(Instruction::Return {
                value: Some(Operand::Var(ret_err)),
            });
        } else {
            // For Option: return None
            let ret_type = &self.current_fn_return_type.clone();
            self.register_adt_type(ret_type);
            let ret_type_name = ret_type.monomorphized_name();
            let none_val = self.fresh_temp();
            body.push(Instruction::AdtInit {
                dest: none_val.clone(),
                type_name: ret_type_name,
                tag: 1,
                fields: vec![],
            });
            // spec §12.3: emit deferred expressions before early return
            self.emit_deferred(body);
            body.push(Instruction::Return {
                value: Some(Operand::Var(none_val)),
            });
        }

        // Success path: extract ok/some payload (field 1)
        body.push(Instruction::Label(ok_label));
        let payload = self.fresh_temp();
        body.push(Instruction::AdtPayload {
            dest: payload.clone(),
            obj: Operand::Var(inner_val),
            type_name,
            field_index: 1,
        });
        // Track the extracted payload type for downstream type inference
        let payload_type = if inner_type.is_option() {
            inner_type.option_inner().cloned()
        } else {
            inner_type.result_ok_type().cloned()
        };
        if let Some(ref pt) = payload_type {
            match pt {
                Ty::String => { self.string_vars.insert(payload.clone()); }
                Ty::Float => { self.float_vars.insert(payload.clone()); }
                Ty::Named(n) => { self.var_types.insert(payload.clone(), n.clone()); }
                Ty::Generic(_, _) => {
                    self.generic_var_types.insert(payload.clone(), pt.clone());
                    self.var_types.insert(payload.clone(), pt.monomorphized_name());
                }
                _ => {}
            }
        }
        payload
    }
}
