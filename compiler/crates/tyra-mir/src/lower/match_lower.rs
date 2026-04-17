// Match expression lowering.
//
// Extracted from mod.rs — lowers `match` expressions into a chain of
// conditional branches with alloca/store/load for the result.

use tyra_ast::*;
use tyra_types::Ty;

use crate::ir::*;

impl super::LowerCtx {
    /// Lower a match expression into a chain of conditional branches.
    /// Uses alloca + store + load pattern for the result to avoid SSA dominance issues.
    pub(super) fn lower_match(&mut self, m: &MatchExpr, body: &mut Vec<Instruction>) -> String {
        let subject = self.lower_expr(&m.subject, body);
        let end_label = self.fresh_label("match_end");

        // Allocate stack slot for match result
        let result_slot = self.fresh_temp();
        body.push(Instruction::Alloca {
            dest: result_slot.clone(),
        });

        // Pre-allocate pattern-bound variables to avoid SSA dominance issues.
        // When multiple arms bind the same name (e.g., Dog(name) + Cat(name)),
        // the alloca must dominate all uses across all arms.
        for arm in &m.arms {
            if let PatternKind::Constructor(_, fields) = &arm.pattern.kind {
                for pf in fields {
                    if !self.mut_vars.contains(&pf.field_name) {
                        body.push(Instruction::Alloca {
                            dest: pf.field_name.clone(),
                        });
                        self.mut_vars.insert(pf.field_name.clone());
                    }
                }
            }
        }

        // Pre-generate all labels
        let arm_labels: Vec<String> = (0..m.arms.len())
            .map(|i| self.fresh_label(&format!("arm_{i}")))
            .collect();
        let next_labels: Vec<String> = (0..m.arms.len())
            .map(|i| {
                if i + 1 < m.arms.len() {
                    self.fresh_label(&format!("next_{i}"))
                } else {
                    end_label.clone()
                }
            })
            .collect();

        for (i, arm) in m.arms.iter().enumerate() {
            let arm_label = &arm_labels[i];
            let next_label = &next_labels[i];

            // Generate pattern test
            match &arm.pattern.kind {
                PatternKind::Wildcard | PatternKind::Ident(_) => {
                    body.push(Instruction::Jump {
                        label: arm_label.clone(),
                    });
                }
                PatternKind::IntLit(n) => {
                    let lit = self.fresh_temp();
                    body.push(Instruction::Const {
                        dest: lit.clone(),
                        value: Constant::Int(*n),
                    });
                    let cond = self.fresh_temp();
                    body.push(Instruction::BinOp {
                        dest: cond.clone(),
                        op: MirBinOp::EqInt,
                        lhs: Operand::Var(subject.clone()),
                        rhs: Operand::Var(lit),
                    });
                    body.push(Instruction::BranchIf {
                        cond: Operand::Var(cond),
                        true_label: arm_label.clone(),
                        false_label: next_label.clone(),
                    });
                }
                PatternKind::BoolLit(b) => {
                    let lit = self.fresh_temp();
                    body.push(Instruction::Const {
                        dest: lit.clone(),
                        value: Constant::Bool(*b),
                    });
                    let cond = self.fresh_temp();
                    body.push(Instruction::BinOp {
                        dest: cond.clone(),
                        op: MirBinOp::EqInt,
                        lhs: Operand::Var(subject.clone()),
                        rhs: Operand::Var(lit),
                    });
                    body.push(Instruction::BranchIf {
                        cond: Operand::Var(cond),
                        true_label: arm_label.clone(),
                        false_label: next_label.clone(),
                    });
                }
                PatternKind::StringLit(s) => {
                    // §11: match on string literal via strcmp
                    let pat_ref = self.intern_string(s);
                    let pat_temp = self.fresh_temp();
                    body.push(Instruction::Const {
                        dest: pat_temp.clone(),
                        value: Constant::StringRef(pat_ref),
                    });
                    let cond = self.fresh_temp();
                    body.push(Instruction::BinOp {
                        dest: cond.clone(),
                        op: MirBinOp::EqString,
                        lhs: Operand::Var(subject.clone()),
                        rhs: Operand::Var(pat_temp),
                    });
                    body.push(Instruction::BranchIf {
                        cond: Operand::Var(cond),
                        true_label: arm_label.clone(),
                        false_label: next_label.clone(),
                    });
                }
                PatternKind::FloatLit(_) => {
                    // Float pattern matching: deferred (Float has no Eq)
                    body.push(Instruction::Jump {
                        label: arm_label.clone(),
                    });
                }
                PatternKind::Constructor(variant_name, pat_fields) => {
                    // Check if this is an Option/Result variant (Some/None/Ok/Err)
                    let prelude_tag = match variant_name.as_str() {
                        "Some" | "Ok" => Some(0i64),
                        "None" | "Err" => Some(1i64),
                        _ => None,
                    };

                    if let Some(tag) = prelude_tag {
                        // Option/Result ADT: extract tag from tagged struct
                        let subject_type_name = self
                            .generic_var_types
                            .get(&subject)
                            .map(|t| t.monomorphized_name())
                            .or_else(|| self.var_types.get(&subject).cloned())
                            .unwrap_or_else(|| {
                                if self.current_fn_return_type.is_option()
                                    || self.current_fn_return_type.is_result()
                                {
                                    self.current_fn_return_type.monomorphized_name()
                                } else {
                                    panic!(
                                        "BUG: cannot determine ADT type for match subject '{subject}'"
                                    )
                                }
                            });

                        let tag_val = self.fresh_temp();
                        body.push(Instruction::AdtTag {
                            dest: tag_val.clone(),
                            obj: Operand::Var(subject.clone()),
                            type_name: subject_type_name.clone(),
                        });
                        let lit = self.fresh_temp();
                        body.push(Instruction::Const {
                            dest: lit.clone(),
                            value: Constant::Int(tag),
                        });
                        let cond = self.fresh_temp();
                        body.push(Instruction::BinOp {
                            dest: cond.clone(),
                            op: MirBinOp::EqInt,
                            lhs: Operand::Var(tag_val),
                            rhs: Operand::Var(lit),
                        });

                        // Check for nested Constructor pattern: Err(NotFound) vs Err(name)
                        // If the inner field is a Constructor, we need to check its tag too.
                        let has_inner_constructor = pat_fields.first().is_some_and(|pf| {
                            matches!(pf.pattern.kind, PatternKind::Constructor(_, _))
                        });

                        if has_inner_constructor {
                            let inner_check = self.fresh_label("inner_check");
                            body.push(Instruction::BranchIf {
                                cond: Operand::Var(cond),
                                true_label: inner_check.clone(),
                                false_label: next_label.clone(),
                            });

                            // Inner tag check: extract payload and compare inner variant tag
                            body.push(Instruction::Label(inner_check));
                            let field_index = if variant_name == "Err" { 2 } else { 1 };
                            let payload = self.fresh_temp();
                            body.push(Instruction::AdtPayload {
                                dest: payload.clone(),
                                obj: Operand::Var(subject.clone()),
                                type_name: subject_type_name,
                                field_index,
                            });

                            if let PatternKind::Constructor(ref inner_variant, _) =
                                pat_fields[0].pattern.kind
                            {
                                // Look up the inner ADT type from the subject's generic type
                                let inner_type_name = self
                                    .generic_var_types
                                    .get(&subject)
                                    .and_then(|ty| {
                                        if variant_name == "Err" {
                                            ty.result_err_type().cloned()
                                        } else if variant_name == "Ok" {
                                            ty.result_ok_type().cloned()
                                        } else {
                                            ty.option_inner().cloned()
                                        }
                                    })
                                    .and_then(|ty| match ty {
                                        Ty::Named(n) => Some(n),
                                        _ => None,
                                    });

                                let inner_tag = inner_type_name
                                    .as_ref()
                                    .and_then(|tn| {
                                        self.variant_tags
                                            .get(&(tn.clone(), inner_variant.clone()))
                                            .copied()
                                    });

                                if let Some(expected_tag) = inner_tag {
                                    // Unit-variant ADT: payload is the tag integer directly.
                                    // NOTE: For struct-based inner ADTs (e.g., NotFound(String)),
                                    // we would need AdtTag extraction instead of EqInt on payload.
                                    let inner_lit = self.fresh_temp();
                                    body.push(Instruction::Const {
                                        dest: inner_lit.clone(),
                                        value: Constant::Int(expected_tag),
                                    });
                                    let inner_cond = self.fresh_temp();
                                    body.push(Instruction::BinOp {
                                        dest: inner_cond.clone(),
                                        op: MirBinOp::EqInt,
                                        lhs: Operand::Var(payload),
                                        rhs: Operand::Var(inner_lit),
                                    });
                                    body.push(Instruction::BranchIf {
                                        cond: Operand::Var(inner_cond),
                                        true_label: arm_label.clone(),
                                        false_label: next_label.clone(),
                                    });
                                } else {
                                    // Could not resolve inner tag — fall through
                                    body.push(Instruction::Jump {
                                        label: arm_label.clone(),
                                    });
                                }
                            }
                        } else {
                            body.push(Instruction::BranchIf {
                                cond: Operand::Var(cond),
                                true_label: arm_label.clone(),
                                false_label: next_label.clone(),
                            });
                        }
                    } else {
                        // User-defined ADT: look up tag from variant_tags.
                        // Use subject type name for disambiguation when available.
                        let subject_type_name = self.var_types.get(&subject).cloned();
                        let tag = if let Some(ref stn) = subject_type_name {
                            self.variant_tags
                                .get(&(stn.clone(), variant_name.clone()))
                                .copied()
                        } else {
                            // Fallback: search by variant name only (ambiguous)
                            self.variant_tags
                                .iter()
                                .find(|((_, vn), _)| vn == variant_name)
                                .map(|(_, &t)| t)
                        };

                        if let Some(tag) = tag {
                            let has_struct = subject_type_name
                                .as_ref()
                                .map(|n| self.adt_struct_defs.contains_key(n))
                                .unwrap_or(false);

                            if has_struct {
                                // Struct-based ADT: extract tag via AdtTag
                                let stn = subject_type_name.unwrap();
                                let tag_val = self.fresh_temp();
                                body.push(Instruction::AdtTag {
                                    dest: tag_val.clone(),
                                    obj: Operand::Var(subject.clone()),
                                    type_name: stn,
                                });
                                let lit = self.fresh_temp();
                                body.push(Instruction::Const {
                                    dest: lit.clone(),
                                    value: Constant::Int(tag),
                                });
                                let cond = self.fresh_temp();
                                body.push(Instruction::BinOp {
                                    dest: cond.clone(),
                                    op: MirBinOp::EqInt,
                                    lhs: Operand::Var(tag_val),
                                    rhs: Operand::Var(lit),
                                });
                                body.push(Instruction::BranchIf {
                                    cond: Operand::Var(cond),
                                    true_label: arm_label.clone(),
                                    false_label: next_label.clone(),
                                });
                            } else {
                                // Unit-only ADT: subject is plain integer tag
                                let lit = self.fresh_temp();
                                body.push(Instruction::Const {
                                    dest: lit.clone(),
                                    value: Constant::Int(tag),
                                });
                                let cond = self.fresh_temp();
                                body.push(Instruction::BinOp {
                                    dest: cond.clone(),
                                    op: MirBinOp::EqInt,
                                    lhs: Operand::Var(subject.clone()),
                                    rhs: Operand::Var(lit),
                                });
                                body.push(Instruction::BranchIf {
                                    cond: Operand::Var(cond),
                                    true_label: arm_label.clone(),
                                    false_label: next_label.clone(),
                                });
                            }
                        } else {
                            // Unknown constructor — fall through (treat as wildcard)
                            body.push(Instruction::Jump {
                                label: arm_label.clone(),
                            });
                        }
                    }
                }
            }

            // Arm body
            body.push(Instruction::Label(arm_label.clone()));

            if let PatternKind::Ident(name) = &arm.pattern.kind {
                body.push(Instruction::Copy {
                    dest: name.clone(),
                    source: subject.clone(),
                });
            }

            // Track arm body start BEFORE pattern bindings so that
            // pattern-bound variables (Copy instructions) are included in
            // last_temp_in_range when the arm body just references the bound variable.
            let arm_body_start = body.len();

            // Bind constructor payload variables: when Some(x) → x = payload
            if let PatternKind::Constructor(variant_name, fields) = &arm.pattern.kind {
                let is_prelude = matches!(variant_name.as_str(), "Some" | "Ok" | "Err");
                // Skip payload binding when inner pattern is a nested Constructor
                // (e.g., Err(NotFound)) — the inner tag was already checked in pattern test.
                let inner_is_constructor = fields.first().is_some_and(|pf| {
                    matches!(pf.pattern.kind, PatternKind::Constructor(_, _))
                });
                if is_prelude && !fields.is_empty()
                    && fields[0].field_name != "_"
                    && !inner_is_constructor
                {
                    let subject_type_name = self
                        .generic_var_types
                        .get(&subject)
                        .map(|t| t.monomorphized_name())
                        .or_else(|| self.var_types.get(&subject).cloned())
                        .unwrap_or_else(|| {
                            if self.current_fn_return_type.is_option()
                                || self.current_fn_return_type.is_result()
                            {
                                self.current_fn_return_type.monomorphized_name()
                            } else {
                                panic!(
                                    "BUG: cannot determine ADT type for match subject '{subject}'"
                                )
                            }
                        });

                    // Extract payload from ADT and bind to the first field variable
                    // For Option: Some=field 1. For Result: Ok=field 1, Err=field 2.
                    let field_index = if variant_name == "Err" { 2 } else { 1 };
                    let payload = self.fresh_temp();
                    body.push(Instruction::AdtPayload {
                        dest: payload.clone(),
                        obj: Operand::Var(subject.clone()),
                        type_name: subject_type_name.clone(),
                        field_index,
                    });

                    // Store into the pre-allocated alloca for this variable
                    let bind_name = &fields[0].field_name;
                    body.push(Instruction::Store {
                        dest: bind_name.clone(),
                        value: Operand::Var(payload),
                    });

                    // Track the type of the bound variable
                    if let Some(subject_ty) = self.generic_var_types.get(&subject) {
                        if let Some(inner) = subject_ty.option_inner() {
                            match inner {
                                Ty::String => { self.string_vars.insert(bind_name.clone()); }
                                Ty::Float => { self.float_vars.insert(bind_name.clone()); }
                                _ => {}
                            }
                        } else if variant_name == "Ok" {
                            if let Some(ok_ty) = subject_ty.result_ok_type() {
                                match ok_ty {
                                    Ty::String => { self.string_vars.insert(bind_name.clone()); }
                                    Ty::Float => { self.float_vars.insert(bind_name.clone()); }
                                    _ => {}
                                }
                            }
                        } else if variant_name == "Err" {
                            if let Some(err_ty) = subject_ty.result_err_type() {
                                match err_ty {
                                    Ty::String => { self.string_vars.insert(bind_name.clone()); }
                                    Ty::Float => { self.float_vars.insert(bind_name.clone()); }
                                    _ => {}
                                }
                            }
                        }
                    }
                } else if !fields.is_empty() {
                    // User-defined ADT: extract payload fields by position
                    let subject_type_name = self.var_types.get(&subject).cloned();
                    if let Some(stn) = subject_type_name {
                        // Look up variant field definitions using subject type name
                        let vfields: Option<Vec<(String, Ty)>> = self
                            .adt_variant_fields
                            .get(&(stn.clone(), variant_name.clone()))
                            .cloned();

                        if let Some(vfields) = vfields {
                            for (fi, pf) in fields.iter().enumerate() {
                                // Skip wildcard bindings
                                if pf.field_name == "_" {
                                    continue;
                                }
                                let field_index = (fi + 1) as u32; // +1 for tag at field 0
                                let payload = self.fresh_temp();
                                body.push(Instruction::AdtPayload {
                                    dest: payload.clone(),
                                    obj: Operand::Var(subject.clone()),
                                    type_name: stn.clone(),
                                    field_index,
                                });
                                // Store into the pre-allocated alloca for this variable
                                body.push(Instruction::Store {
                                    dest: pf.field_name.clone(),
                                    value: Operand::Var(payload.clone()),
                                });
                                // Track field type
                                if let Some((_, fty)) = vfields.get(fi) {
                                    match fty {
                                        Ty::String => {
                                            self.string_vars.insert(pf.field_name.clone());
                                        }
                                        Ty::Float => {
                                            self.float_vars.insert(pf.field_name.clone());
                                        }
                                        _ => {}
                                    }
                                }
                            }
                        }
                    }
                }
            }

            for stmt in &arm.body {
                self.lower_stmt(stmt, body);
            }

            // Store arm result into the alloca'd slot (scan only this arm's instructions)
            if let Some(last) = self.last_temp_in_range(body, arm_body_start) {
                body.push(Instruction::Store {
                    dest: result_slot.clone(),
                    value: Operand::Var(last),
                });
            }

            body.push(Instruction::Jump {
                label: end_label.clone(),
            });

            // Next arm label
            if i + 1 < m.arms.len() {
                body.push(Instruction::Label(next_label.clone()));
            }
        }

        body.push(Instruction::Label(end_label));

        // Load the result from the alloca'd slot
        let result = self.fresh_temp();
        body.push(Instruction::Load {
            dest: result.clone(),
            source: result_slot,
        });
        result
    }
}
