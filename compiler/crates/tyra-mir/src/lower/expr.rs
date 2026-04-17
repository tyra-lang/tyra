// Expression lowering — extracted from mod.rs.
//
// Contains the `lower_expr` method which flattens AST expressions
// into named temporaries and MIR instructions.

use tyra_ast::*;
use tyra_types::Ty;

use crate::ir::*;

impl super::LowerCtx {
    /// Lower an expression, returning the name of the temporary holding the result.
    pub(super) fn lower_expr(&mut self, expr: &Expr, body: &mut Vec<Instruction>) -> String {
        match &expr.kind {
            ExprKind::IntLit(n) => {
                let dest = self.fresh_temp();
                body.push(Instruction::Const {
                    dest: dest.clone(),
                    value: Constant::Int(*n),
                });
                dest
            }
            ExprKind::FloatLit(f) => {
                let dest = self.fresh_temp();
                body.push(Instruction::Const {
                    dest: dest.clone(),
                    value: Constant::Float(*f),
                });
                dest
            }
            ExprKind::StringLit(s) => {
                let idx = self.intern_string(s);
                let dest = self.fresh_temp();
                body.push(Instruction::Const {
                    dest: dest.clone(),
                    value: Constant::StringRef(idx),
                });
                dest
            }
            ExprKind::BoolLit(b) => {
                let dest = self.fresh_temp();
                body.push(Instruction::Const {
                    dest: dest.clone(),
                    value: Constant::Bool(*b),
                });
                dest
            }
            ExprKind::UnitLit => {
                let dest = self.fresh_temp();
                body.push(Instruction::Const {
                    dest: dest.clone(),
                    value: Constant::Unit,
                });
                dest
            }

            ExprKind::Ident(name) => {
                // Check for None constructor
                if name == "None" {
                    // Infer the Option<T> type from context (function return type or let binding)
                    let full_type = if self.current_fn_return_type.is_option() {
                        self.current_fn_return_type.clone()
                    } else {
                        // Fallback: Option<Int>
                        Ty::Generic("Option".into(), vec![Ty::Int])
                    };
                    self.register_adt_type(&full_type);
                    let type_name = full_type.monomorphized_name();

                    let dest = self.fresh_temp();
                    body.push(Instruction::AdtInit {
                        dest: dest.clone(),
                        type_name: type_name.clone(),
                        tag: 1,
                        fields: vec![],
                    });
                    self.generic_var_types.insert(dest.clone(), full_type);
                    self.var_types.insert(dest.clone(), type_name);
                    return dest;
                }

                if self.mut_vars.contains(name.as_str()) {
                    // Mutable local: load from alloca
                    let temp = self.fresh_temp();
                    body.push(Instruction::Load {
                        dest: temp.clone(),
                        source: name.clone(),
                    });
                    temp
                } else {
                    name.clone()
                }
            }

            ExprKind::BinaryOp(lhs, op, rhs) => {
                let l = self.lower_expr(lhs, body);
                let r = self.lower_expr(rhs, body);

                // String comparison: use strcmp-based ops
                let is_string = self.is_string_expr(lhs) || self.is_string_expr(rhs);
                if is_string && matches!(op, BinOp::Eq | BinOp::NotEq) {
                    let mir_op = if *op == BinOp::Eq {
                        MirBinOp::EqString
                    } else {
                        MirBinOp::NeqString
                    };
                    let dest = self.fresh_temp();
                    body.push(Instruction::BinOp {
                        dest: dest.clone(),
                        op: mir_op,
                        lhs: Operand::Var(l),
                        rhs: Operand::Var(r),
                    });
                    return dest;
                }

                // Value type comparison: extract fields and compare
                if let Some(dest) = self.lower_value_type_binop(
                    &l, &r, *op, lhs, rhs, body,
                ) {
                    return dest;
                }

                // Default: Int/Float/Bool comparison
                let dest = self.fresh_temp();
                let is_float = self.is_float_expr(lhs) || self.is_float_expr(rhs);
                let mir_op = super::ast_binop_to_mir(*op, is_float);
                body.push(Instruction::BinOp {
                    dest: dest.clone(),
                    op: mir_op,
                    lhs: Operand::Var(l),
                    rhs: Operand::Var(r),
                });
                dest
            }

            ExprKind::UnaryOp(op, operand) => {
                let val = self.lower_expr(operand, body);
                let dest = self.fresh_temp();
                match op {
                    UnaryOp::Neg => {
                        body.push(Instruction::Neg {
                            dest: dest.clone(),
                            operand: Operand::Var(val),
                        });
                    }
                    UnaryOp::Not => {
                        body.push(Instruction::Not {
                            dest: dest.clone(),
                            operand: Operand::Var(val),
                        });
                    }
                }
                dest
            }

            ExprKind::Call(callee, args) => {
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
                    // Determine if receiver is Option<T>
                    let opt_type = self.infer_expr_type(obj)
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
                        let obj_val = self.lower_expr(obj, body);
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
                            body.push(Instruction::Call {
                                dest: Some(dest.clone()),
                                func: qualified_name,
                                args: arg_operands,
                            });
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

            ExprKind::Assign(lhs, rhs) => {
                let val = self.lower_expr(rhs, body);
                match &lhs.kind {
                    ExprKind::Ident(name) => {
                        if self.mut_vars.contains(name.as_str()) {
                            // Mutable local: store to alloca
                            body.push(Instruction::Store {
                                dest: name.clone(),
                                value: Operand::Var(val.clone()),
                            });
                        } else {
                            body.push(Instruction::Copy {
                                dest: name.clone(),
                                source: val.clone(),
                            });
                        }
                    }
                    ExprKind::FieldAccess(obj, field) => {
                        // Field mutation: obj.field = val
                        if let ExprKind::Ident(obj_name) = &obj.kind {
                            if self.mut_vars.contains(obj_name.as_str()) {
                                self.lower_field_assign(
                                    obj_name, obj, field, &val, body,
                                );
                            }
                        }
                    }
                    _ => {}
                }
                val
            }

            ExprKind::If(if_expr) => self.lower_if(if_expr, body),

            ExprKind::Match(m) => self.lower_match(m, body),

            ExprKind::For(f) => {
                let iter_val = self.lower_expr(&f.iter, body);

                // Detect if iterating over a List
                let list_type = self
                    .generic_var_types
                    .get(&iter_val)
                    .filter(|ty| ty.is_list())
                    .cloned();

                if let Some(list_ty) = list_type {
                    let elem_type = list_ty.list_elem().cloned().unwrap_or(Ty::Int);

                    // Get length
                    let len = self.fresh_temp();
                    body.push(Instruction::ListLen {
                        dest: len.clone(),
                        list: Operand::Var(iter_val.clone()),
                    });

                    // mut i = 0
                    let idx_var = self.fresh_temp();
                    body.push(Instruction::Alloca {
                        dest: idx_var.clone(),
                    });
                    let zero = self.fresh_temp();
                    body.push(Instruction::Const {
                        dest: zero.clone(),
                        value: Constant::Int(0),
                    });
                    body.push(Instruction::Store {
                        dest: idx_var.clone(),
                        value: Operand::Var(zero),
                    });

                    let loop_label = self.fresh_label("for");
                    let body_label = format!("{loop_label}_body");
                    let end_label = self.fresh_label("for_end");

                    // Jump into loop header
                    body.push(Instruction::Jump {
                        label: loop_label.clone(),
                    });

                    // Loop header: check i < len
                    body.push(Instruction::Label(loop_label.clone()));
                    let cur_idx = self.fresh_temp();
                    body.push(Instruction::Load {
                        dest: cur_idx.clone(),
                        source: idx_var.clone(),
                    });
                    let cond = self.fresh_temp();
                    body.push(Instruction::BinOp {
                        dest: cond.clone(),
                        op: MirBinOp::LtInt,
                        lhs: Operand::Var(cur_idx.clone()),
                        rhs: Operand::Var(len.clone()),
                    });
                    body.push(Instruction::BranchIf {
                        cond: Operand::Var(cond),
                        true_label: body_label.clone(),
                        false_label: end_label.clone(),
                    });

                    // Loop body: binding = list[i]
                    body.push(Instruction::Label(body_label));
                    let elem = self.fresh_temp();
                    body.push(Instruction::ListGet {
                        dest: elem.clone(),
                        list: Operand::Var(iter_val),
                        index: Operand::Var(cur_idx.clone()),
                        elem_type: elem_type.clone(),
                    });
                    // Track element type for codegen (Bool tracked in codegen pre-scan)
                    match &elem_type {
                        Ty::String => {
                            self.string_vars.insert(f.binding.clone());
                        }
                        Ty::Float => {
                            self.float_vars.insert(f.binding.clone());
                        }
                        _ => {}
                    }
                    body.push(Instruction::Copy {
                        dest: f.binding.clone(),
                        source: elem,
                    });

                    // User's loop body
                    for stmt in &f.body {
                        self.lower_stmt(stmt, body);
                    }

                    // Increment: i = i + 1
                    let one = self.fresh_temp();
                    body.push(Instruction::Const {
                        dest: one.clone(),
                        value: Constant::Int(1),
                    });
                    let next_idx = self.fresh_temp();
                    body.push(Instruction::BinOp {
                        dest: next_idx.clone(),
                        op: MirBinOp::AddInt,
                        lhs: Operand::Var(cur_idx),
                        rhs: Operand::Var(one),
                    });
                    body.push(Instruction::Store {
                        dest: idx_var,
                        value: Operand::Var(next_idx),
                    });
                    body.push(Instruction::Jump {
                        label: loop_label,
                    });

                    // End
                    body.push(Instruction::Label(end_label));
                } else {
                    // Non-list iteration: keep current stub behavior
                    body.push(Instruction::Copy {
                        dest: f.binding.clone(),
                        source: iter_val,
                    });
                    for stmt in &f.body {
                        self.lower_stmt(stmt, body);
                    }
                }

                let dest = self.fresh_temp();
                body.push(Instruction::Const {
                    dest: dest.clone(),
                    value: Constant::Unit,
                });
                dest
            }

            ExprKind::While(w) => {
                let loop_label = self.fresh_label("while");
                let end_label = self.fresh_label("while_end");

                body.push(Instruction::Label(loop_label.clone()));
                let cond = self.lower_expr(&w.condition, body);
                body.push(Instruction::BranchIf {
                    cond: Operand::Var(cond),
                    true_label: format!("{loop_label}_body"),
                    false_label: end_label.clone(),
                });
                body.push(Instruction::Label(format!("{loop_label}_body")));
                for stmt in &w.body {
                    self.lower_stmt(stmt, body);
                }
                body.push(Instruction::Jump { label: loop_label });
                body.push(Instruction::Label(end_label));

                let dest = self.fresh_temp();
                body.push(Instruction::Const {
                    dest: dest.clone(),
                    value: Constant::Unit,
                });
                dest
            }

            ExprKind::Propagate(inner) => {
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
                                    eprintln!(
                                        "warning: ? operator on Result<_, {}> in function returning Result<_, {}> — no Into<{}> impl found for {}",
                                        inner_err.display_name(),
                                        ret_err.display_name(),
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

            ExprKind::Await(inner) => {
                // .await: simplified, just lower the inner expression
                self.lower_expr(inner, body)
            }

            ExprKind::Spawn(inner) => self.lower_expr(inner, body),

            ExprKind::FieldAccess(obj, field) => {
                // Check if this is an ADT constructor: Color.Red or Payment.Cash
                if let ExprKind::Ident(type_name) = &obj.kind
                    && let Some(&tag) = self.variant_tags.get(&(type_name.clone(), field.clone()))
                {
                    // If this ADT has a struct def (has payload variants), emit AdtInit
                    if self.adt_struct_defs.contains_key(type_name) {
                        let max_field_count = self.adt_struct_defs[type_name].len() - 1;
                        let fields = vec![Operand::Const(Constant::Int(0)); max_field_count];
                        let dest = self.fresh_temp();
                        body.push(Instruction::AdtInit {
                            dest: dest.clone(),
                            type_name: type_name.clone(),
                            tag,
                            fields,
                        });
                        self.var_types.insert(dest.clone(), type_name.clone());
                        return dest;
                    }
                    // Pure unit-only ADT: emit tag constant directly
                    let dest = self.fresh_temp();
                    body.push(Instruction::Const {
                        dest: dest.clone(),
                        value: Constant::Int(tag),
                    });
                    return dest;
                }

                let obj_val = self.lower_expr(obj, body);

                // Value type field access: emit FieldGet instruction
                if let Some((type_name, field_defs)) = self.resolve_struct_type(obj) {
                    if let Some(idx) = field_defs.iter().position(|(n, _)| n == field) {
                        let dest = self.fresh_temp();
                        body.push(Instruction::FieldGet {
                            dest: dest.clone(),
                            obj: Operand::Var(obj_val),
                            type_name,
                            field_index: idx as u32,
                        });
                        return dest;
                    }
                }

                // General field access (data types, methods)
                // TODO: emit proper GEP instruction for data type struct field access
                let dest = self.fresh_temp();
                body.push(Instruction::Copy {
                    dest: dest.clone(),
                    source: format!("{obj_val}.{field}"),
                });
                dest
            }

            ExprKind::Index(obj, idx) => {
                // §11: items[index] — panicking index access
                let obj_val = self.lower_expr(obj, body);
                let idx_val = self.lower_expr(idx, body);

                // Determine element type from tracking
                let elem_type = self
                    .generic_var_types
                    .get(&obj_val)
                    .and_then(|ty| ty.list_elem().cloned())
                    .unwrap_or(Ty::Int);

                let dest = self.fresh_temp();
                body.push(Instruction::ListGet {
                    dest: dest.clone(),
                    list: Operand::Var(obj_val),
                    index: Operand::Var(idx_val),
                    elem_type: elem_type.clone(),
                });

                // Propagate element type to the result temp
                match &elem_type {
                    Ty::String => {
                        self.string_vars.insert(dest.clone());
                    }
                    Ty::Float => {
                        self.float_vars.insert(dest.clone());
                    }
                    Ty::Named(n) => {
                        self.var_types.insert(dest.clone(), n.clone());
                    }
                    _ => {}
                }
                dest
            }

            ExprKind::Lambda(_) => {
                let dest = self.fresh_temp();
                body.push(Instruction::Const {
                    dest: dest.clone(),
                    value: Constant::Unit,
                });
                dest
            }

            ExprKind::TurbofishCall(callee, type_args, args) => {
                // spec §8.4: turbofish call — monomorphize generic function
                if let ExprKind::Ident(fn_name) = &callee.kind {
                    let concrete_types: Vec<Ty> = type_args
                        .iter()
                        .map(Ty::from_type_expr)
                        .collect();

                    // Monomorphize and get mangled name
                    let mangled = self.monomorphize(fn_name, &concrete_types);

                    if let Some(func_name) = mangled {
                        // Lower arguments
                        let arg_operands: Vec<Operand> = args
                            .iter()
                            .map(|a| {
                                let t = self.lower_expr(&a.value, body);
                                Operand::Var(t)
                            })
                            .collect();

                        let dest = self.fresh_temp();
                        // Infer return type for type tracking
                        let ret_ty = self.fn_return_types.get(&func_name).cloned();
                        body.push(Instruction::Call {
                            dest: Some(dest.clone()),
                            func: func_name,
                            args: arg_operands,
                        });
                        // Track result type
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
                // Fallback: unresolved turbofish — no generic function found
                eprintln!("warning: unresolved turbofish call (no matching generic function)");
                let dest = self.fresh_temp();
                body.push(Instruction::Const {
                    dest: dest.clone(),
                    value: Constant::Unit,
                });
                dest
            }

            ExprKind::ListLit(items) => {
                // §11: List literal [a, b, c]
                let elem_operands: Vec<Operand> = items
                    .iter()
                    .map(|item| {
                        let t = self.lower_expr(item, body);
                        Operand::Var(t)
                    })
                    .collect();

                // Infer element type from first item (default Int for empty)
                let elem_type = if let Some(first) = items.first() {
                    self.infer_expr_type(first).unwrap_or(Ty::Int)
                } else {
                    Ty::Int
                };

                let list_type = Ty::Generic("List".into(), vec![elem_type.clone()]);
                self.register_adt_type(&list_type);
                let type_name = list_type.monomorphized_name();

                let dest = self.fresh_temp();
                body.push(Instruction::ListInit {
                    dest: dest.clone(),
                    elem_type,
                    elements: elem_operands,
                });

                // Track as a generic type for downstream use
                self.generic_var_types.insert(dest.clone(), list_type);
                self.var_types.insert(dest.clone(), type_name);
                dest
            }

            ExprKind::MapLit(entries) => {
                for (k, v) in entries {
                    self.lower_expr(k, body);
                    self.lower_expr(v, body);
                }
                let dest = self.fresh_temp();
                body.push(Instruction::Const {
                    dest: dest.clone(),
                    value: Constant::Unit,
                });
                dest
            }

            ExprKind::StringInterp(parts) => {
                // Build a printf-style format string and collect args.
                // Type detection determines the format specifier per expression.
                let mut format_str = String::new();
                let mut format_args: Vec<Operand> = Vec::new();

                for part in parts {
                    match part {
                        StringPart::Lit(s) => {
                            // Escape '%' for printf format strings
                            format_str.push_str(&s.replace('%', "%%"));
                        }
                        StringPart::Expr(e) => {
                            let is_float = self.is_float_expr(e);
                            let is_string = self.is_string_expr(e);
                            let val = self.lower_expr(e, body);

                            if is_string {
                                format_str.push_str("%s");
                            } else if is_float {
                                format_str.push_str("%g");
                            } else {
                                format_str.push_str("%ld");
                            }
                            format_args.push(Operand::Var(val));
                        }
                    }
                }

                let format_ref = self.intern_string(&format_str);
                let dest = self.fresh_temp();
                body.push(Instruction::StringFormat {
                    dest: dest.clone(),
                    format_ref,
                    args: format_args,
                });
                dest
            }
        }
    }
}
