// Expression lowering — extracted from mod.rs.
//
// Contains the `lower_expr` method which flattens AST expressions
// into named temporaries and MIR instructions.
#![allow(clippy::collapsible_if, clippy::collapsible_else_if)]

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
                    // Infer the Option<T> type from context. Priority:
                    //   1. Active `let x: Option<T> = None` annotation
                    //      hint — most specific.
                    //   2. Enclosing function's `Option<T>` return type.
                    //   3. Fallback to `Option<Int>` (Tyra v0.1 has no
                    //      first-class type variables yet).
                    let full_type = if let Some(hint) =
                        self.binding_type_hint.as_ref().filter(|t| t.is_option())
                    {
                        hint.clone()
                    } else if self.current_fn_return_type.is_option() {
                        self.current_fn_return_type.clone()
                    } else {
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

                if self.mut_vars.contains(name.as_str())
                    || self.pattern_vars.contains(name.as_str())
                {
                    // Alloca-backed variable (mutable or pattern-bound): load from alloca
                    let temp = self.fresh_temp();
                    body.push(Instruction::Load {
                        dest: temp.clone(),
                        source: name.clone(),
                    });
                    // Propagate string/float tracking through load
                    if self.string_vars.contains(name.as_str()) {
                        self.string_vars.insert(temp.clone());
                    }
                    if self.float_vars.contains(name.as_str()) {
                        self.float_vars.insert(temp.clone());
                    }
                    // M9 follow-up: propagate Task<T> tracking through load
                    // so `mut t = spawn f(); ... t.await` unboxes correctly.
                    if let Some(trt) = self.task_result_types.get(name.as_str()).cloned() {
                        self.task_result_types.insert(temp.clone(), trt);
                    }
                    // Propagate struct / ADT type tracking through load so
                    // downstream consumers (match subject ADT lookup, method
                    // dispatch) can resolve the type from the fresh Load temp.
                    // Without this, a `let x = Option<Int>; ...; match x` where
                    // `x` was alloca-backed (multi-let shadow or mut) produces
                    // a Load temp that looks type-less to match_lower.
                    if let Some(vty) = self.var_types.get(name.as_str()).cloned() {
                        self.var_types.insert(temp.clone(), vty);
                    }
                    if let Some(gt) = self.generic_var_types.get(name.as_str()).cloned() {
                        self.generic_var_types.insert(temp.clone(), gt);
                    }
                    temp
                } else {
                    name.clone()
                }
            }

            ExprKind::BinaryOp(lhs, op, rhs) => {
                let l = self.lower_expr(lhs, body);
                // Propagate ADT type from lhs to rhs so `line != None` where
                // `line: Option<String>` constructs None as Option<String>
                // rather than the default Option<Int>.
                let lhs_adt_hint = self
                    .generic_var_types
                    .get(&l)
                    .filter(|t| t.is_option() || t.is_result())
                    .cloned();
                let prev_hint = self.binding_type_hint.clone();
                if let Some(ref adt) = lhs_adt_hint {
                    self.binding_type_hint = Some(adt.clone());
                }
                let r = self.lower_expr(rhs, body);
                self.binding_type_hint = prev_hint;

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
                if let Some(dest) = self.lower_value_type_binop(&l, &r, *op, lhs, rhs, body) {
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
                            // Propagate string/float type on reassignment
                            if self.string_vars.contains(&val) {
                                self.string_vars.insert(name.clone());
                            }
                            if self.float_vars.contains(&val) {
                                self.float_vars.insert(name.clone());
                            }
                            // Propagate Task<T> handle tracking on reassignment
                            // so `mut t = spawn f(); t = spawn g(); t.await`
                            // unboxes correctly (M9 follow-up).
                            if let Some(trt) = self.task_result_types.get(&val).cloned() {
                                self.task_result_types.insert(name.clone(), trt);
                            }
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
                                self.lower_field_assign(obj_name, obj, field, &val, body);
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
                    // Record the for-loop induction variable as a local
                    // binding regardless of its element type. The type-
                    // keyed maps below only track String / Float / Named /
                    // Generic; Int / Bool / Unit bindings would otherwise
                    // leak through shadow detection.
                    self.local_binding_names.insert(f.binding.clone());
                    // Track element type for codegen (Bool tracked in codegen pre-scan).
                    // Named/Generic bindings must also register in var_types /
                    // generic_var_types so subsequent `binding.field` /
                    // `binding[i]` accesses resolve.
                    match &elem_type {
                        Ty::String => {
                            self.string_vars.insert(f.binding.clone());
                        }
                        Ty::Float => {
                            self.float_vars.insert(f.binding.clone());
                        }
                        Ty::Named(n) => {
                            self.var_types.insert(f.binding.clone(), n.clone());
                        }
                        Ty::Generic(_, _) => {
                            self.generic_var_types
                                .insert(f.binding.clone(), elem_type.clone());
                            self.var_types
                                .insert(f.binding.clone(), elem_type.monomorphized_name());
                        }
                        _ => {}
                    }
                    // If the binding name is already slotted (hoisted
                    // pattern/mut alloca, or a prior `let`/`mut` of the same
                    // name), reuse the slot via Store. Emitting Copy here
                    // would mint a fresh SSA `%binding` and collide with the
                    // existing alloca — E0500.
                    //
                    // Invariant: when this guard fires, the pre-existing
                    // slot's LLVM type must match `elem_type`. The type
                    // checker rejects shadowing at incompatible types in
                    // the same function scope, so an outer `let x: Foo`
                    // followed by `for x in int_list` is a prior type
                    // error — we never reach here. If that invariant ever
                    // weakens, Store will silently produce mistyped IR and
                    // LLVM will emit a type-mismatch E0500; tighten with a
                    // MIR-level assert at that point.
                    if self.pattern_vars.contains(&f.binding) || self.mut_vars.contains(&f.binding)
                    {
                        body.push(Instruction::Store {
                            dest: f.binding.clone(),
                            value: Operand::Var(elem),
                        });
                    } else {
                        body.push(Instruction::Copy {
                            dest: f.binding.clone(),
                            source: elem,
                        });
                    }

                    // User's loop body.
                    // `continue` must jump past the user body to the increment,
                    // so introduce a dedicated continue_label before the increment.
                    let continue_label = self.fresh_label("for_continue");
                    self.loop_exit_stack.push(end_label.clone());
                    self.loop_head_stack.push(continue_label.clone());
                    for stmt in &f.body {
                        self.lower_stmt(stmt, body);
                    }
                    self.loop_head_stack.pop();
                    self.loop_exit_stack.pop();
                    // Explicit jump to terminate the body block (required by LLVM IR).
                    // Dead code if the body already ended with break/return, which is
                    // handled the same way as while's unconditional back-edge jump.
                    body.push(Instruction::Jump {
                        label: continue_label.clone(),
                    });
                    body.push(Instruction::Label(continue_label));

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
                    body.push(Instruction::Jump { label: loop_label });

                    // End
                    body.push(Instruction::Label(end_label));
                } else {
                    // Non-list iteration: keep current stub behavior.
                    // Same invariant as the list branch: a pre-existing
                    // slot for `f.binding` must be type-compatible with
                    // `iter_val`. Upheld by the type checker today.
                    let stub_end = self.fresh_label("for_end");
                    self.local_binding_names.insert(f.binding.clone());
                    if self.pattern_vars.contains(&f.binding) || self.mut_vars.contains(&f.binding)
                    {
                        body.push(Instruction::Store {
                            dest: f.binding.clone(),
                            value: Operand::Var(iter_val),
                        });
                    } else {
                        body.push(Instruction::Copy {
                            dest: f.binding.clone(),
                            source: iter_val,
                        });
                    }
                    let stub_continue = self.fresh_label("for_continue");
                    self.loop_exit_stack.push(stub_end.clone());
                    self.loop_head_stack.push(stub_continue.clone());
                    for stmt in &f.body {
                        self.lower_stmt(stmt, body);
                    }
                    self.loop_head_stack.pop();
                    self.loop_exit_stack.pop();
                    body.push(Instruction::Jump {
                        label: stub_continue.clone(),
                    });
                    body.push(Instruction::Label(stub_continue));
                    body.push(Instruction::Jump {
                        label: stub_end.clone(),
                    });
                    body.push(Instruction::Label(stub_end));
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

                // LLVM basic blocks require an explicit terminator; jump
                // into the loop header rather than falling through from
                // the surrounding block (the previous block may end in an
                // alloca/store sequence with no terminator).
                body.push(Instruction::Jump {
                    label: loop_label.clone(),
                });
                body.push(Instruction::Label(loop_label.clone()));
                let cond = self.lower_expr(&w.condition, body);
                body.push(Instruction::BranchIf {
                    cond: Operand::Var(cond),
                    true_label: format!("{loop_label}_body"),
                    false_label: end_label.clone(),
                });
                body.push(Instruction::Label(format!("{loop_label}_body")));
                self.loop_exit_stack.push(end_label.clone());
                self.loop_head_stack.push(loop_label.clone());
                for stmt in &w.body {
                    self.lower_stmt(stmt, body);
                }
                self.loop_head_stack.pop();
                self.loop_exit_stack.pop();
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
                // §14.3 + M9: If the inner expression produces a live Task<T>
                // handle (tracked via task_result_types), emit a real Await
                // instruction. Otherwise (async-as-sync stub, §14 v0.1), fall
                // through to identity — the value is already the final T.
                //
                // Tracking is propagated through:
                //   - let-binding Copy          (Stmt::Let, mod.rs)
                //   - mut-binding Alloca/Store  (Stmt::Mut, mod.rs)
                //   - mut reassignment          (ExprKind::Assign above)
                //   - Ident Load from alloca    (ExprKind::Ident above)
                // For-loop `for t in tasks` and match-pattern bindings
                // are still unsupported — callers that need to await an
                // element should index the list or use `tasks.join_all`.
                let task_temp = self.lower_expr(inner, body);
                if let Some(result_type) = self.task_result_types.get(&task_temp).cloned() {
                    let dest = self.fresh_temp();
                    body.push(Instruction::Await {
                        dest: dest.clone(),
                        task: Operand::Var(task_temp),
                        result_type: result_type.clone(),
                    });
                    // Re-register the unboxed value with the underlying type
                    // so downstream propagate/match see the true ADT.
                    if matches!(&result_type, Ty::Generic(_, _)) {
                        self.generic_var_types
                            .insert(dest.clone(), result_type.clone());
                    }
                    self.var_types
                        .insert(dest.clone(), result_type.monomorphized_name());
                    dest
                } else {
                    task_temp
                }
            }

            ExprKind::Spawn(inner) => {
                // §14.4 + M9: `spawn f(args)` submits a task to the runtime
                // scheduler. The inner expression must be a function call.
                // Record the arg and return types on the Spawn instruction so
                // codegen can build per-site thunks and result boxes.
                if let ExprKind::Call(callee, call_args) = &inner.kind
                    && let ExprKind::Ident(fn_name) = &callee.kind
                    && let Some(arg_types) = self.fn_param_types.get(fn_name).cloned()
                    && let Some(result_type) = self.fn_return_types.get(fn_name).cloned()
                {
                    // Nested tasks (spawn f() where f returns Task<T>) are
                    // not supported in v0.1 — the thunk would box a Task
                    // handle as the "result" and the outer await would
                    // silently skip the inner one. Reject at lowering time
                    // rather than miscompile.
                    if let Ty::Generic(name, _) = &result_type
                        && name == "Task"
                    {
                        panic!(
                            "spawn f() where f returns Task<T> is not supported in v0.1 \
                             (nested tasks). Fn: {fn_name}"
                        );
                    }
                    let args: Vec<Operand> = call_args
                        .iter()
                        .map(|a| Operand::Var(self.lower_expr(&a.value, body)))
                        .collect();
                    let dest = self.fresh_temp();
                    body.push(Instruction::Spawn {
                        dest: dest.clone(),
                        func: fn_name.clone(),
                        args,
                        arg_types,
                        result_type: result_type.clone(),
                    });
                    // Track the Task<T> result type separately from
                    // generic_var_types so downstream ?/match/list ops still
                    // see the underlying T when the task is eventually
                    // awaited. (See task_result_types on LowerCtx.)
                    self.task_result_types.insert(dest.clone(), result_type);
                    dest
                } else {
                    // Fallback: lower as sync call (pre-M9 behavior).
                    self.lower_expr(inner, body)
                }
            }

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
                                self.generic_var_types
                                    .insert(dest.clone(), field_ty.clone());
                                self.var_types
                                    .insert(dest.clone(), field_ty.monomorphized_name());
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
                        self.generic_var_types
                            .insert(dest.clone(), elem_type.clone());
                        self.var_types
                            .insert(dest.clone(), elem_type.monomorphized_name());
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
                    let concrete_types: Vec<Ty> =
                        type_args.iter().map(Ty::from_type_expr).collect();

                    // Built-in turbofish functions (parse::<T>)
                    if fn_name == "parse" && concrete_types.len() == 1 {
                        let mangled_name =
                            format!("parse__{}", concrete_types[0].monomorphized_name());
                        let ret_ty = Ty::Generic("Option".into(), vec![concrete_types[0].clone()]);
                        self.register_adt_type(&ret_ty);
                        self.fn_return_types
                            .insert(mangled_name.clone(), ret_ty.clone());

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
                        self.var_types
                            .insert(dest.clone(), ret_ty.monomorphized_name());
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
                //
                // Peel one level off the active binding hint before
                // recursing into items so a nested empty literal (e.g. the
                // `[]` inside `let data: List<List<Int>> = [[1,2],[]]`)
                // sees `List<Int>` as its hint, not the outer
                // `List<List<Int>>`. Without this, the inner `[]` is
                // typed as the outer element type and the outer list's
                // `insertvalue` trips an LLVM struct-type mismatch.
                let peeled_hint = self
                    .binding_type_hint
                    .as_ref()
                    .filter(|t| t.is_list())
                    .and_then(|t| t.list_elem().cloned());
                let prev_hint = self.binding_type_hint.clone();
                if peeled_hint.is_some() {
                    self.binding_type_hint = peeled_hint.clone();
                }
                let elem_operands: Vec<Operand> = items
                    .iter()
                    .map(|item| {
                        let t = self.lower_expr(item, body);
                        Operand::Var(t)
                    })
                    .collect();
                self.binding_type_hint = prev_hint;

                // Infer element type from first item, or from the active
                // binding annotation hint when the literal is empty
                // (`mut xs: List<String> = []`). Without the hint an empty
                // list defaults to `List<Int>` and the subsequent `Store`
                // into the annotated `List<String>` slot trips E0500.
                let elem_type = if let Some(first) = items.first() {
                    self.infer_expr_type(first).unwrap_or(Ty::Int)
                } else if let Some(hint) = peeled_hint {
                    hint
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
                // §17.3.6 v0.1: Map<String, Int> only. Build via
                //   handle = __map_new_string_int()
                //   handle = __map_insert_string_int(handle, k, v)  (×N)
                //   wrap in Map__String__Int { handle }
                // Type checker has already rejected non-(String, Int) shapes.
                let map_ty = Ty::Generic("Map".into(), vec![Ty::String, Ty::Int]);
                self.register_adt_type(&map_ty);

                // Start with an empty handle.
                let mut handle = self.fresh_temp();
                body.push(Instruction::Call {
                    dest: Some(handle.clone()),
                    func: "__map_new_string_int".into(),
                    args: vec![],
                });
                self.string_vars.insert(handle.clone()); // ptr-typed

                for (k, v) in entries {
                    let k_val = self.lower_expr(k, body);
                    let v_val = self.lower_expr(v, body);
                    let next = self.fresh_temp();
                    body.push(Instruction::Call {
                        dest: Some(next.clone()),
                        func: "__map_insert_string_int".into(),
                        args: vec![
                            Operand::Var(handle.clone()),
                            Operand::Var(k_val),
                            Operand::Var(v_val),
                        ],
                    });
                    self.string_vars.insert(next.clone());
                    handle = next;
                }

                // Wrap the handle in Map__String__Int { handle }.
                let dest = self.fresh_temp();
                body.push(Instruction::StructInit {
                    dest: dest.clone(),
                    type_name: "Map__String__Int".into(),
                    fields: vec![Operand::Var(handle)],
                });
                self.var_types
                    .insert(dest.clone(), "Map__String__Int".into());
                self.generic_var_types.insert(dest.clone(), map_ty);
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
