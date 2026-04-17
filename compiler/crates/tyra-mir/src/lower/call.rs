// Call expression lowering — extracted from expr.rs.
//
// Contains the `lower_call` method which handles all ExprKind::Call
// variants: constructors, method calls, module-qualified calls, etc.

use tyra_ast::*;
use tyra_types::Ty;

use crate::ir::*;

impl super::LowerCtx {
    /// Lower a call expression, returning the name of the temporary holding the result.
    pub(super) fn lower_call(
        &mut self,
        callee: &Expr,
        args: &[Arg],
        body: &mut Vec<Instruction>,
    ) -> String {
        // Check for Option/Result constructors: Some(x), Ok(x), Err(e)
        if let ExprKind::Ident(ctor_name) = &callee.kind
            && matches!(ctor_name.as_str(), "Some" | "Ok" | "Err")
            && args.len() == 1
        {
            let arg_val = self.lower_expr(&args[0].value, body);
            let arg_type = self.infer_expr_type(&args[0].value).unwrap_or(Ty::Int);
            let tag = if ctor_name == "Err" { 1i64 } else { 0i64 };

            let full_type = self
                .infer_adt_call_type(ctor_name, &arg_type)
                .unwrap_or_else(|| Ty::Generic("Option".into(), vec![arg_type]));
            self.register_adt_type(&full_type);
            let type_name = full_type.monomorphized_name();

            // Build fields vector based on constructor type
            let fields = match ctor_name.as_str() {
                "Some" => vec![Operand::Var(arg_val)],
                "Ok" => vec![Operand::Var(arg_val), Operand::Const(Constant::Int(0))],
                "Err" => vec![Operand::Const(Constant::Int(0)), Operand::Var(arg_val)],
                _ => vec![Operand::Var(arg_val)],
            };

            let dest = self.fresh_temp();
            body.push(Instruction::AdtInit {
                dest: dest.clone(),
                type_name: type_name.clone(),
                tag,
                fields,
            });
            self.generic_var_types.insert(dest.clone(), full_type);
            self.var_types.insert(dest.clone(), type_name);
            return dest;
        }

        // Check for qualified ADT constructor: Payment.Card(last4: "1234")
        if let ExprKind::FieldAccess(obj, variant_name) = &callee.kind
            && let ExprKind::Ident(type_name) = &obj.kind
            && self.adt_variant_fields.contains_key(&(type_name.clone(), variant_name.clone()))
        {
            let vfields = self
                .adt_variant_fields
                .get(&(type_name.clone(), variant_name.clone()))
                .cloned()
                .unwrap_or_default();
            let tag = self
                .variant_tags
                .get(&(type_name.clone(), variant_name.clone()))
                .copied()
                .unwrap_or(0);

            // Map labeled args to field order (same logic as value constructors)
            let max_field_count = self
                .adt_struct_defs
                .get(type_name)
                .map(|f| f.len() - 1) // subtract tag field
                .unwrap_or(vfields.len());

            let mut field_operands = Vec::with_capacity(max_field_count);
            let mut used_args: std::collections::HashSet<usize> =
                std::collections::HashSet::new();

            for (_fi, (fname, _fty)) in vfields.iter().enumerate() {
                let labeled = args.iter().enumerate().find(|(idx, a)| {
                    !used_args.contains(idx) && a.label.as_deref() == Some(fname)
                });
                let resolved = if let Some((idx, a)) = labeled {
                    used_args.insert(idx);
                    Some(a)
                } else {
                    let positional =
                        args.iter().enumerate().find(|(idx, _)| !used_args.contains(idx));
                    if let Some((idx, a)) = positional {
                        used_args.insert(idx);
                        Some(a)
                    } else {
                        None
                    }
                };
                if let Some(a) = resolved {
                    let val = self.lower_expr(&a.value, body);
                    field_operands.push(Operand::Var(val));
                } else {
                    field_operands.push(Operand::Const(Constant::Int(0)));
                }
            }

            // Pad with zeros for fields beyond this variant's count
            while field_operands.len() < max_field_count {
                field_operands.push(Operand::Const(Constant::Int(0)));
            }

            let dest = self.fresh_temp();
            body.push(Instruction::AdtInit {
                dest: dest.clone(),
                type_name: type_name.clone(),
                tag,
                fields: field_operands,
            });
            self.var_types.insert(dest.clone(), type_name.clone());
            return dest;
        }

        // Check for value type constructor: Point(x: 3.0, y: 4.0)
        if let ExprKind::Ident(name) = &callee.kind
            && self.struct_fields.contains_key(name)
        {
            let field_defs = self.struct_fields[name].clone();
            // Map labeled args to declaration order.
            // If args have labels, match by label name.
            // If no labels, assume positional order.
            let mut field_operands = Vec::with_capacity(field_defs.len());
            let mut used_args: std::collections::HashSet<usize> = std::collections::HashSet::new();
            for (fname, _fty) in &field_defs {
                // First try label match
                let labeled = args.iter().enumerate().find(|(idx, a)| {
                    !used_args.contains(idx) && a.label.as_deref() == Some(fname)
                });
                let resolved = if let Some((idx, a)) = labeled {
                    used_args.insert(idx);
                    Some(a)
                } else {
                    // Positional fallback: next unused arg
                    let positional = args.iter().enumerate().find(|(idx, _)| {
                        !used_args.contains(idx)
                    });
                    if let Some((idx, a)) = positional {
                        used_args.insert(idx);
                        Some(a)
                    } else {
                        None
                    }
                };
                if let Some(a) = resolved {
                    let val = self.lower_expr(&a.value, body);
                    field_operands.push(Operand::Var(val));
                } else {
                    // Missing field — emit unit as placeholder
                    field_operands.push(Operand::Const(Constant::Unit));
                }
            }
            let dest = self.fresh_temp();
            body.push(Instruction::StructInit {
                dest: dest.clone(),
                type_name: name.clone(),
                fields: field_operands,
            });
            return dest;
        }

        // Check for .len() on List<T> (spec §11)
        if let ExprKind::FieldAccess(obj, method) = &callee.kind
            && method == "len"
            && args.is_empty()
        {
            if let Some(_list_ty) = self.infer_list_type(obj) {
                let obj_val = self.lower_expr(obj, body);
                let dest = self.fresh_temp();
                body.push(Instruction::ListLen {
                    dest: dest.clone(),
                    list: Operand::Var(obj_val),
                });
                return dest;
            }
        }

        // Check for .get(index) on List<T> (spec §11)
        if let ExprKind::FieldAccess(obj, method) = &callee.kind
            && method == "get"
            && args.len() == 1
        {
            if let Some(list_ty) = self.infer_list_type(obj) {
                let elem_type = list_ty.list_elem().cloned().unwrap_or(Ty::Int);
                let obj_val = self.lower_expr(obj, body);
                let idx_val = self.lower_expr(&args[0].value, body);

                let option_type = Ty::Generic("Option".into(), vec![elem_type.clone()]);
                self.register_adt_type(&option_type);
                let option_type_name = option_type.monomorphized_name();

                let dest = self.fresh_temp();
                body.push(Instruction::ListGetSafe {
                    dest: dest.clone(),
                    list: Operand::Var(obj_val),
                    index: Operand::Var(idx_val),
                    elem_type,
                });

                // Track the result as Option<T>
                self.generic_var_types.insert(dest.clone(), option_type);
                self.var_types.insert(dest.clone(), option_type_name);
                return dest;
            }
        }

        // Check for .ok_or() on Option<T> (spec §12.2):
        // Converts Option<T> to Result<T, E> where E is the type of the argument.
        if let ExprKind::FieldAccess(obj, method) = &callee.kind
            && method == "ok_or"
            && args.len() == 1
        {
            // Lower receiver first so chained calls (e.g., .get().ok_or()) are tracked
            let obj_val = self.lower_expr(obj, body);

            // Determine if receiver is Option<T> (check lowered temp in generic_var_types)
            let opt_type = self.generic_var_types.get(&obj_val).cloned()
                .or_else(|| self.infer_expr_type(obj))
                .or_else(|| {
                    if let ExprKind::Ident(name) = &obj.kind {
                        self.generic_var_types.get(name).cloned()
                    } else {
                        None
                    }
                });
            if let Some(ref oty) = opt_type
                && oty.is_option()
            {
                let err_arg = self.lower_expr(&args[0].value, body);

                // Infer err type from the argument expression, variable
                // tracking, or the enclosing function's return type
                let err_type = self.infer_expr_type(&args[0].value)
                    .or_else(|| {
                        self.var_types.get(&err_arg).map(|n| Ty::Named(n.clone()))
                    })
                    .or_else(|| {
                        self.current_fn_return_type.result_err_type().cloned()
                    })
                    .unwrap_or(Ty::Named("Error".into()));

                let inner_t = oty.option_inner().cloned().unwrap_or(Ty::Int);
                let result_type = Ty::Generic(
                    "Result".into(),
                    vec![inner_t, err_type],
                );
                self.register_adt_type(&result_type);
                let result_type_name = result_type.monomorphized_name();
                let opt_type_name = oty.monomorphized_name();

                // Extract tag from Option
                let tag = self.fresh_temp();
                body.push(Instruction::AdtTag {
                    dest: tag.clone(),
                    obj: Operand::Var(obj_val.clone()),
                    type_name: opt_type_name.clone(),
                });

                // Check: tag == 0 means Some
                let zero = self.fresh_temp();
                body.push(Instruction::Const {
                    dest: zero.clone(),
                    value: Constant::Int(0),
                });
                let is_some = self.fresh_temp();
                body.push(Instruction::BinOp {
                    dest: is_some.clone(),
                    op: MirBinOp::EqInt,
                    lhs: Operand::Var(tag),
                    rhs: Operand::Var(zero),
                });

                let some_label = self.fresh_label("ok_or_some");
                let none_label = self.fresh_label("ok_or_none");
                let end_label = self.fresh_label("ok_or_end");

                // Allocate result slot
                let result_slot = self.fresh_temp();
                body.push(Instruction::Alloca {
                    dest: result_slot.clone(),
                });

                body.push(Instruction::BranchIf {
                    cond: Operand::Var(is_some),
                    true_label: some_label.clone(),
                    false_label: none_label.clone(),
                });

                // Some path: Ok(payload)
                body.push(Instruction::Label(some_label));
                let payload = self.fresh_temp();
                body.push(Instruction::AdtPayload {
                    dest: payload.clone(),
                    obj: Operand::Var(obj_val),
                    type_name: opt_type_name,
                    field_index: 1,
                });
                let ok_val = self.fresh_temp();
                body.push(Instruction::AdtInit {
                    dest: ok_val.clone(),
                    type_name: result_type_name.clone(),
                    tag: 0,
                    fields: vec![
                        Operand::Var(payload),
                        Operand::Const(Constant::Int(0)),
                    ],
                });
                body.push(Instruction::Store {
                    dest: result_slot.clone(),
                    value: Operand::Var(ok_val),
                });
                body.push(Instruction::Jump {
                    label: end_label.clone(),
                });

                // None path: Err(err_arg)
                body.push(Instruction::Label(none_label));
                let err_val = self.fresh_temp();
                body.push(Instruction::AdtInit {
                    dest: err_val.clone(),
                    type_name: result_type_name,
                    tag: 1,
                    fields: vec![
                        Operand::Const(Constant::Int(0)),
                        Operand::Var(err_arg),
                    ],
                });
                body.push(Instruction::Store {
                    dest: result_slot.clone(),
                    value: Operand::Var(err_val),
                });
                body.push(Instruction::Jump {
                    label: end_label.clone(),
                });

                body.push(Instruction::Label(end_label));
                let result_val = self.fresh_temp();
                body.push(Instruction::Load {
                    dest: result_val.clone(),
                    source: result_slot,
                });
                // Track the result as a generic type for downstream ? operator
                self.generic_var_types
                    .insert(result_val.clone(), result_type.clone());
                self.var_types
                    .insert(result_val.clone(), result_type.monomorphized_name());
                return result_val;
            }
            // Receiver was lowered but is not Option — treat as generic method call.
            // Use obj_val directly to avoid double-lowering.
            let arg_operands: Vec<Operand> = args
                .iter()
                .map(|a| {
                    let t = self.lower_expr(&a.value, body);
                    Operand::Var(t)
                })
                .collect();
            let dest = self.fresh_temp();
            let mangled = format!("{obj_val}.ok_or");
            body.push(Instruction::Call {
                dest: Some(dest.clone()),
                func: mangled,
                args: arg_operands,
            });
            return dest;
        }

        // Check for .copy() on value types only (§8.6)
        // copy() is NOT available on data types.
        if let ExprKind::FieldAccess(obj, method) = &callee.kind
            && method == "copy"
        {
            if let Some((type_name, field_defs)) = self.resolve_struct_type(obj) {
                if !self.data_types.contains(&type_name) {
                    let obj_val = self.lower_expr(obj, body);
                    return self.lower_copy(
                        &obj_val, &type_name, &field_defs, args, body,
                    );
                }
            }
            // Not a value type — fall through to method call or generic call
        }

        // Check for impl trait method call: p.method() → Type__method(p)
        // (§8.7 static dispatch)
        if let ExprKind::FieldAccess(obj, method) = &callee.kind {
            match self.resolve_impl_method(obj, method) {
                super::ImplMethodResult::Resolved(mangled_name) => {
                    let self_val = self.lower_expr(obj, body);
                    let mut arg_operands = vec![Operand::Var(self_val)];
                    for a in args {
                        let t = self.lower_expr(&a.value, body);
                        arg_operands.push(Operand::Var(t));
                    }
                    let dest = self.fresh_temp();
                    body.push(Instruction::Call {
                        dest: Some(dest.clone()),
                        func: mangled_name,
                        args: arg_operands,
                    });
                    return dest;
                }
                super::ImplMethodResult::Ambiguous => {
                    // Multiple impls define this method but type can't be resolved.
                    // Emit a call to a clearly-invalid name to produce a linker error
                    // rather than silently generating broken IR.
                    // TODO: Emit proper diagnostic via tyra-diagnostics.
                    let self_val = self.lower_expr(obj, body);
                    let dest = self.fresh_temp();
                    body.push(Instruction::Call {
                        dest: Some(dest.clone()),
                        func: format!("__unresolved_method_{method}"),
                        args: vec![Operand::Var(self_val)],
                    });
                    return dest;
                }
                super::ImplMethodResult::NotFound => {
                    // Not an impl method — fall through
                }
            }
        }

        // Check for module-qualified call: math.square() → math__square() (§13)
        if let ExprKind::FieldAccess(obj, fn_name) = &callee.kind {
            if let ExprKind::Ident(module_name) = &obj.kind {
                if self.imported_modules.contains(module_name.as_str()) {
                    let qualified_name = format!("{module_name}__{fn_name}");
                    let arg_operands: Vec<Operand> = args
                        .iter()
                        .map(|a| {
                            let t = self.lower_expr(&a.value, body);
                            Operand::Var(t)
                        })
                        .collect();
                    let dest = self.fresh_temp();
                    // Track return type from fn_return_types
                    let ret_ty = self.fn_return_types.get(&qualified_name).cloned();
                    body.push(Instruction::Call {
                        dest: Some(dest.clone()),
                        func: qualified_name,
                        args: arg_operands,
                    });
                    if let Some(ref ty) = ret_ty {
                        match ty {
                            Ty::String => { self.string_vars.insert(dest.clone()); }
                            Ty::Float => { self.float_vars.insert(dest.clone()); }
                            Ty::Named(n) => { self.var_types.insert(dest.clone(), n.clone()); }
                            Ty::Generic(_, _) => {
                                self.generic_var_types.insert(dest.clone(), ty.clone());
                                self.var_types.insert(dest.clone(), ty.monomorphized_name());
                            }
                            _ => {}
                        }
                    }
                    return dest;
                }
            }
        }

        // Special case: print/println/eprint/eprintln with StringInterp argument.
        // Emit separate print calls for each segment.
        if let ExprKind::Ident(fname) = &callee.kind
            && matches!(fname.as_str(), "print" | "println" | "eprint" | "eprintln")
            && args.len() == 1
            && let ExprKind::StringInterp(parts) = &args[0].value.kind
        {
            let is_println = fname == "println" || fname == "eprintln";
            for part in parts {
                match part {
                    StringPart::Lit(s) => {
                        let idx = self.intern_string(s);
                        let str_temp = self.fresh_temp();
                        body.push(Instruction::Const {
                            dest: str_temp.clone(),
                            value: Constant::StringRef(idx),
                        });
                        body.push(Instruction::Call {
                            dest: None,
                            func: "print".into(),
                            args: vec![Operand::Var(str_temp)],
                        });
                    }
                    StringPart::Expr(e) => {
                        let val = self.lower_expr(e, body);
                        body.push(Instruction::Call {
                            dest: None,
                            func: "print".into(),
                            args: vec![Operand::Var(val)],
                        });
                    }
                }
            }
            // Add newline for println/eprintln
            if is_println {
                let nl_idx = self.intern_string("\n");
                let nl_temp = self.fresh_temp();
                body.push(Instruction::Const {
                    dest: nl_temp.clone(),
                    value: Constant::StringRef(nl_idx),
                });
                body.push(Instruction::Call {
                    dest: None,
                    func: "print".into(),
                    args: vec![Operand::Var(nl_temp)],
                });
            }
            let dest = self.fresh_temp();
            body.push(Instruction::Const {
                dest: dest.clone(),
                value: Constant::Unit,
            });
            return dest;
        }

        let func_name = match &callee.kind {
            ExprKind::Ident(name) => name.clone(),
            ExprKind::FieldAccess(obj, method) => {
                let obj_name = self.lower_expr(obj, body);
                format!("{obj_name}.{method}")
            }
            _ => self.lower_expr(callee, body),
        };

        let arg_operands: Vec<Operand> = args
            .iter()
            .map(|a| {
                let t = self.lower_expr(&a.value, body);
                Operand::Var(t)
            })
            .collect();

        let dest = self.fresh_temp();
        body.push(Instruction::Call {
            dest: Some(dest.clone()),
            func: func_name.clone(),
            args: arg_operands,
        });

        // Track generic return types from function signatures
        if let Some(ret_ty) = self.fn_return_types.get(&func_name).cloned() {
            if ret_ty.is_option() || ret_ty.is_result() {
                self.register_adt_type(&ret_ty);
                let mono = ret_ty.monomorphized_name();
                self.generic_var_types.insert(dest.clone(), ret_ty);
                self.var_types.insert(dest.clone(), mono);
            }
        }

        dest
    }
}
