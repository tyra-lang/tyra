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

            ExprKind::Call(callee, args) => self.lower_call(callee, args, body),

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

            ExprKind::Propagate(inner) => self.lower_propagate(inner, body),

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
                        let field_ty = field_defs[idx].1.clone();
                        let dest = self.fresh_temp();
                        body.push(Instruction::FieldGet {
                            dest: dest.clone(),
                            obj: Operand::Var(obj_val),
                            type_name,
                            field_index: idx as u32,
                        });
                        // Track field type so downstream callers can infer it correctly
                        match &field_ty {
                            Ty::String => { self.string_vars.insert(dest.clone()); }
                            Ty::Float => { self.float_vars.insert(dest.clone()); }
                            Ty::Named(n) => { self.var_types.insert(dest.clone(), n.clone()); }
                            Ty::Generic(_, _) => {
                                self.generic_var_types.insert(dest.clone(), field_ty.clone());
                                self.var_types.insert(dest.clone(), field_ty.monomorphized_name());
                            }
                            _ => {}
                        }
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
                    Ty::Generic(_, _) => {
                        self.generic_var_types.insert(dest.clone(), elem_type.clone());
                        self.var_types.insert(dest.clone(), elem_type.monomorphized_name());
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

                    // Built-in turbofish functions (parse::<T>)
                    if fn_name == "parse" && concrete_types.len() == 1 {
                        let mangled_name = format!("parse__{}", concrete_types[0].monomorphized_name());
                        let ret_ty = Ty::Generic("Option".into(), vec![concrete_types[0].clone()]);
                        self.register_adt_type(&ret_ty);
                        self.fn_return_types.insert(mangled_name.clone(), ret_ty.clone());

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
                            func: mangled_name,
                            args: arg_operands,
                        });
                        self.generic_var_types.insert(dest.clone(), ret_ty.clone());
                        self.var_types.insert(dest.clone(), ret_ty.monomorphized_name());
                        return dest;
                    }

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
