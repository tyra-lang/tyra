// Match expression lowering.
//
// Extracted from mod.rs — lowers `match` expressions into a chain of
// conditional branches with alloca/store/load for the result.
#![allow(clippy::collapsible_if, clippy::collapsible_else_if)]
#![allow(clippy::single_match)]

use tyra_ast::*;
use tyra_types::Ty;

use crate::ir::*;

impl super::LowerCtx<'_> {
    /// Lower a match expression into a chain of conditional branches.
    /// Uses alloca + store + load pattern for the result to avoid SSA dominance issues.
    pub(super) fn lower_match(&mut self, m: &MatchExpr, body: &mut Vec<MirStmt>) -> String {
        let subject = self.lower_expr(&m.subject, body);
        let end_label = self.fresh_label("match_end");

        // Allocate stack slot for match result
        let result_slot = self.fresh_temp();
        self.emit_synthetic(
            body,
            Instruction::Alloca {
                dest: result_slot.clone(),
            },
        );
        let mut result_slot_is_string = false;
        let mut result_slot_is_float = false;
        let mut result_slot_var_type: Option<String> = None;
        let mut result_slot_generic_type: Option<Ty> = None;

        // Pre-allocate pattern-bound variables to avoid SSA dominance issues.
        // When multiple arms bind the same name (e.g., Dog(name) + Cat(name)),
        // the alloca must dominate all uses across all arms.
        for arm in &m.arms {
            self.pre_alloca_pattern_vars(&arm.pattern.kind, body);
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
                    self.emit_synthetic(
                        body,
                        Instruction::Jump {
                            label: arm_label.clone(),
                        },
                    );
                }
                PatternKind::IntLit(n) => {
                    let lit = self.fresh_temp();
                    self.emit(
                        body,
                        Instruction::Const {
                            dest: lit.clone(),
                            value: Constant::Int(*n),
                        },
                    );
                    let cond = self.fresh_temp();
                    self.emit(
                        body,
                        Instruction::BinOp {
                            dest: cond.clone(),
                            op: MirBinOp::EqInt,
                            lhs: Operand::Var(subject.clone()),
                            rhs: Operand::Var(lit),
                        },
                    );
                    self.emit_synthetic(
                        body,
                        Instruction::BranchIf {
                            cond: Operand::Var(cond),
                            true_label: arm_label.clone(),
                            false_label: next_label.clone(),
                        },
                    );
                }
                PatternKind::BoolLit(b) => {
                    let lit = self.fresh_temp();
                    self.emit(
                        body,
                        Instruction::Const {
                            dest: lit.clone(),
                            value: Constant::Bool(*b),
                        },
                    );
                    let cond = self.fresh_temp();
                    self.emit(
                        body,
                        Instruction::BinOp {
                            dest: cond.clone(),
                            op: MirBinOp::EqInt,
                            lhs: Operand::Var(subject.clone()),
                            rhs: Operand::Var(lit),
                        },
                    );
                    self.emit_synthetic(
                        body,
                        Instruction::BranchIf {
                            cond: Operand::Var(cond),
                            true_label: arm_label.clone(),
                            false_label: next_label.clone(),
                        },
                    );
                }
                PatternKind::StringLit(s) => {
                    // §11: match on string literal via strcmp
                    let pat_ref = self.intern_string(s);
                    let pat_temp = self.fresh_temp();
                    self.emit(
                        body,
                        Instruction::Const {
                            dest: pat_temp.clone(),
                            value: Constant::StringRef(pat_ref),
                        },
                    );
                    let cond = self.fresh_temp();
                    self.emit(
                        body,
                        Instruction::BinOp {
                            dest: cond.clone(),
                            op: MirBinOp::EqString,
                            lhs: Operand::Var(subject.clone()),
                            rhs: Operand::Var(pat_temp),
                        },
                    );
                    self.emit_synthetic(
                        body,
                        Instruction::BranchIf {
                            cond: Operand::Var(cond),
                            true_label: arm_label.clone(),
                            false_label: next_label.clone(),
                        },
                    );
                }
                PatternKind::FloatLit(_) => {
                    // Float pattern matching: deferred (Float has no Eq)
                    self.emit_synthetic(
                        body,
                        Instruction::Jump {
                            label: arm_label.clone(),
                        },
                    );
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
                        let subject_type_name = match self
                            .generic_var_types
                            .get(&subject)
                            .map(|t| t.monomorphized_name())
                            .or_else(|| self.var_types.get(&subject).cloned())
                        {
                            Some(n) => n,
                            None if self.current_fn_return_type.is_option()
                                || self.current_fn_return_type.is_result() =>
                            {
                                // Guard against family mismatch: Option pattern on Result fn
                                // or vice versa should not inherit the fn return type.
                                // Also skip when the subject is a known scalar (String/Float)
                                // — using the fn return type for a scalar subject causes an
                                // LLVM extractvalue type mismatch (ptr vs struct).
                                if self.string_vars.contains(&subject)
                                    || self.float_vars.contains(&subject)
                                {
                                    // Fall through to scalar fallback below.
                                    let fallback = match variant_name.as_str() {
                                        "Ok" | "Err" => {
                                            Ty::Generic("Result".into(), vec![Ty::Int, Ty::String])
                                        }
                                        _ => Ty::Generic("Option".into(), vec![Ty::Int]),
                                    };
                                    self.register_adt_type(&fallback);
                                    fallback.monomorphized_name()
                                } else {
                                    let fn_is_option = self.current_fn_return_type.is_option();
                                    let pat_is_option =
                                        matches!(variant_name.as_str(), "Some" | "None");
                                    if fn_is_option == pat_is_option {
                                        self.current_fn_return_type.monomorphized_name()
                                    } else {
                                        let fallback = match variant_name.as_str() {
                                            "Ok" | "Err" => Ty::Generic(
                                                "Result".into(),
                                                vec![Ty::Int, Ty::String],
                                            ),
                                            _ => Ty::Generic("Option".into(), vec![Ty::Int]),
                                        };
                                        self.register_adt_type(&fallback);
                                        fallback.monomorphized_name()
                                    }
                                }
                            }
                            None => {
                                // Graceful fallback when upstream type
                                // inference failed to tag the subject
                                // (typical cause: a method call on an
                                // unknown-typed receiver, e.g. an
                                // unsupported `Map` literal's `.get()`).
                                // Register and emit code assuming the most
                                // common Option/Result variant; downstream
                                // type_scan or LLVM will reject the program
                                // with a clear E0500 type-mismatch rather
                                // than crashing the compiler process.
                                //
                                // Long-term: plumb a Report into MIR
                                // lowering and emit an E0XXX diagnostic
                                // here instead of silently continuing.
                                let fallback = match variant_name.as_str() {
                                    "Ok" | "Err" => {
                                        Ty::Generic("Result".into(), vec![Ty::Int, Ty::String])
                                    }
                                    _ => Ty::Generic("Option".into(), vec![Ty::Int]),
                                };
                                self.register_adt_type(&fallback);
                                fallback.monomorphized_name()
                            }
                        };

                        let tag_val = self.fresh_temp();
                        self.emit(
                            body,
                            Instruction::AdtTag {
                                dest: tag_val.clone(),
                                obj: Operand::Var(subject.clone()),
                                type_name: subject_type_name.clone(),
                            },
                        );
                        let lit = self.fresh_temp();
                        self.emit(
                            body,
                            Instruction::Const {
                                dest: lit.clone(),
                                value: Constant::Int(tag),
                            },
                        );
                        let cond = self.fresh_temp();
                        self.emit(
                            body,
                            Instruction::BinOp {
                                dest: cond.clone(),
                                op: MirBinOp::EqInt,
                                lhs: Operand::Var(tag_val),
                                rhs: Operand::Var(lit),
                            },
                        );

                        // Check for nested Constructor pattern: Err(NotFound) vs Err(name)
                        // If the inner field is a Constructor, we need to check its tag too.
                        let has_inner_constructor = pat_fields.first().is_some_and(|pf| {
                            matches!(pf.pattern.kind, PatternKind::Constructor(_, _))
                        });

                        if has_inner_constructor {
                            let inner_check = self.fresh_label("inner_check");
                            self.emit_synthetic(
                                body,
                                Instruction::BranchIf {
                                    cond: Operand::Var(cond),
                                    true_label: inner_check.clone(),
                                    false_label: next_label.clone(),
                                },
                            );

                            // Inner tag check: extract payload and compare inner variant tag
                            self.emit_synthetic(body, Instruction::Label(inner_check));
                            let field_index = if variant_name == "Err" { 2 } else { 1 };
                            let payload = self.fresh_temp();
                            self.emit(
                                body,
                                Instruction::AdtPayload {
                                    dest: payload.clone(),
                                    obj: Operand::Var(subject.clone()),
                                    type_name: subject_type_name,
                                    field_index,
                                },
                            );

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

                                let inner_tag = inner_type_name.as_ref().and_then(|tn| {
                                    self.variant_tags
                                        .get(&(tn.clone(), inner_variant.clone()))
                                        .copied()
                                });

                                if let Some(expected_tag) = inner_tag {
                                    let itn = inner_type_name.as_ref().unwrap().clone();
                                    let is_inner_struct =
                                        self.adt_struct_defs.contains_key(itn.as_str());

                                    let (tag_subject, payload_for_recurse) = if is_inner_struct {
                                        let inner_tag_val = self.fresh_temp();
                                        self.emit(
                                            body,
                                            Instruction::AdtTag {
                                                dest: inner_tag_val.clone(),
                                                obj: Operand::Var(payload.clone()),
                                                type_name: itn.clone(),
                                            },
                                        );
                                        (inner_tag_val, payload)
                                    } else {
                                        (payload.clone(), payload)
                                    };

                                    let inner_lit = self.fresh_temp();
                                    self.emit(
                                        body,
                                        Instruction::Const {
                                            dest: inner_lit.clone(),
                                            value: Constant::Int(expected_tag),
                                        },
                                    );
                                    let inner_cond = self.fresh_temp();
                                    self.emit(
                                        body,
                                        Instruction::BinOp {
                                            dest: inner_cond.clone(),
                                            op: MirBinOp::EqInt,
                                            lhs: Operand::Var(tag_subject),
                                            rhs: Operand::Var(inner_lit),
                                        },
                                    );

                                    // Check if inner pattern has extractable payload fields.
                                    // Only meaningful when the inner ADT is struct-based.
                                    let inner_fields_ref =
                                        if let PatternKind::Constructor(_, ref ifields) =
                                            pat_fields[0].pattern.kind
                                        {
                                            ifields
                                        } else {
                                            unreachable!()
                                        };
                                    let has_deeper = is_inner_struct
                                        && inner_fields_ref.iter().any(|pf| {
                                            !matches!(pf.pattern.kind, PatternKind::Wildcard)
                                        });

                                    if has_deeper {
                                        let inner_ok = self.fresh_label("inner_ok");
                                        self.emit_synthetic(
                                            body,
                                            Instruction::BranchIf {
                                                cond: Operand::Var(inner_cond),
                                                true_label: inner_ok.clone(),
                                                false_label: next_label.clone(),
                                            },
                                        );
                                        self.emit_synthetic(body, Instruction::Label(inner_ok));
                                        let inner_fields_cloned = inner_fields_ref.clone();
                                        let inner_variant_clone = inner_variant.clone();
                                        let next_label_clone = next_label.clone();
                                        self.lower_ctor_payload_and_vars(
                                            &payload_for_recurse,
                                            &itn,
                                            &inner_variant_clone,
                                            &inner_fields_cloned,
                                            &next_label_clone,
                                            body,
                                        );
                                        self.emit_synthetic(
                                            body,
                                            Instruction::Jump {
                                                label: arm_label.clone(),
                                            },
                                        );
                                    } else {
                                        self.emit_synthetic(
                                            body,
                                            Instruction::BranchIf {
                                                cond: Operand::Var(inner_cond),
                                                true_label: arm_label.clone(),
                                                false_label: next_label.clone(),
                                            },
                                        );
                                    }
                                } else {
                                    // Could not resolve inner tag — fall through
                                    self.emit_synthetic(
                                        body,
                                        Instruction::Jump {
                                            label: arm_label.clone(),
                                        },
                                    );
                                }
                            }
                        } else if let Some(pf) = pat_fields.first() {
                            // Inner pattern is a literal (e.g. `when Some(10)`): after the
                            // tag check, extract the payload and compare with the literal
                            // value. Wildcard / Ident skip the value comparison.
                            let inner_lit_const: Option<Constant> = match &pf.pattern.kind {
                                PatternKind::IntLit(n) => Some(Constant::Int(*n)),
                                // Bool must use Constant::Bool so the temp is tracked in
                                // bool_temps and emitted as i1, not i64 (avoids icmp i1/i64 mismatch).
                                PatternKind::BoolLit(b) => Some(Constant::Bool(*b)),
                                _ => None,
                            };
                            if let Some(lit_const) = inner_lit_const {
                                // tag matched → extract payload → compare with literal
                                let inner_check = self.fresh_label("inner_lit_check");
                                self.emit_synthetic(
                                    body,
                                    Instruction::BranchIf {
                                        cond: Operand::Var(cond),
                                        true_label: inner_check.clone(),
                                        false_label: next_label.clone(),
                                    },
                                );
                                self.emit_synthetic(body, Instruction::Label(inner_check));
                                let field_index = if variant_name == "Err" { 2 } else { 1 };
                                let payload = self.fresh_temp();
                                self.emit(
                                    body,
                                    Instruction::AdtPayload {
                                        dest: payload.clone(),
                                        obj: Operand::Var(subject.clone()),
                                        type_name: subject_type_name,
                                        field_index,
                                    },
                                );
                                let lit_temp = self.fresh_temp();
                                self.emit(
                                    body,
                                    Instruction::Const {
                                        dest: lit_temp.clone(),
                                        value: lit_const,
                                    },
                                );
                                let val_cond = self.fresh_temp();
                                self.emit(
                                    body,
                                    Instruction::BinOp {
                                        dest: val_cond.clone(),
                                        op: MirBinOp::EqInt,
                                        lhs: Operand::Var(payload),
                                        rhs: Operand::Var(lit_temp),
                                    },
                                );
                                self.emit_synthetic(
                                    body,
                                    Instruction::BranchIf {
                                        cond: Operand::Var(val_cond),
                                        true_label: arm_label.clone(),
                                        false_label: next_label.clone(),
                                    },
                                );
                            } else {
                                // Ident / Wildcard inner — tag match is sufficient
                                self.emit_synthetic(
                                    body,
                                    Instruction::BranchIf {
                                        cond: Operand::Var(cond),
                                        true_label: arm_label.clone(),
                                        false_label: next_label.clone(),
                                    },
                                );
                            }
                        } else {
                            self.emit_synthetic(
                                body,
                                Instruction::BranchIf {
                                    cond: Operand::Var(cond),
                                    true_label: arm_label.clone(),
                                    false_label: next_label.clone(),
                                },
                            );
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
                                self.emit(
                                    body,
                                    Instruction::AdtTag {
                                        dest: tag_val.clone(),
                                        obj: Operand::Var(subject.clone()),
                                        type_name: stn,
                                    },
                                );
                                let lit = self.fresh_temp();
                                self.emit(
                                    body,
                                    Instruction::Const {
                                        dest: lit.clone(),
                                        value: Constant::Int(tag),
                                    },
                                );
                                let cond = self.fresh_temp();
                                self.emit(
                                    body,
                                    Instruction::BinOp {
                                        dest: cond.clone(),
                                        op: MirBinOp::EqInt,
                                        lhs: Operand::Var(tag_val),
                                        rhs: Operand::Var(lit),
                                    },
                                );
                                self.emit_synthetic(
                                    body,
                                    Instruction::BranchIf {
                                        cond: Operand::Var(cond),
                                        true_label: arm_label.clone(),
                                        false_label: next_label.clone(),
                                    },
                                );
                            } else {
                                // Unit-only ADT: subject is plain integer tag
                                let lit = self.fresh_temp();
                                self.emit(
                                    body,
                                    Instruction::Const {
                                        dest: lit.clone(),
                                        value: Constant::Int(tag),
                                    },
                                );
                                let cond = self.fresh_temp();
                                self.emit(
                                    body,
                                    Instruction::BinOp {
                                        dest: cond.clone(),
                                        op: MirBinOp::EqInt,
                                        lhs: Operand::Var(subject.clone()),
                                        rhs: Operand::Var(lit),
                                    },
                                );
                                self.emit_synthetic(
                                    body,
                                    Instruction::BranchIf {
                                        cond: Operand::Var(cond),
                                        true_label: arm_label.clone(),
                                        false_label: next_label.clone(),
                                    },
                                );
                            }
                        } else {
                            // Unknown constructor — fall through (treat as wildcard)
                            self.emit_synthetic(
                                body,
                                Instruction::Jump {
                                    label: arm_label.clone(),
                                },
                            );
                        }
                    }
                }
            }

            // Arm body
            self.emit_synthetic(body, Instruction::Label(arm_label.clone()));

            if let PatternKind::Ident(name) = &arm.pattern.kind {
                self.emit(
                    body,
                    Instruction::Copy {
                        dest: name.clone(),
                        source: subject.clone(),
                    },
                );
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
                let inner_is_constructor = fields
                    .iter()
                    .any(|pf| matches!(pf.pattern.kind, PatternKind::Constructor(_, _)));
                // Skip payload binding when inner pattern is a literal or wildcard
                // (e.g., `when Some(10)`) — there is no named variable to bind into,
                // and emitting `Store { dest: "" }` generates malformed LLVM IR.
                let inner_is_literal = !fields.is_empty()
                    && matches!(
                        fields[0].pattern.kind,
                        PatternKind::IntLit(_)
                            | PatternKind::FloatLit(_)
                            | PatternKind::StringLit(_)
                            | PatternKind::BoolLit(_)
                            | PatternKind::Wildcard
                    );
                if is_prelude
                    && !fields.is_empty()
                    && fields[0].field_name != "_"
                    && !inner_is_constructor
                    && !inner_is_literal
                {
                    let subject_type_name = match self
                        .generic_var_types
                        .get(&subject)
                        .map(|t| t.monomorphized_name())
                        .or_else(|| self.var_types.get(&subject).cloned())
                    {
                        Some(n) => n,
                        None if self.current_fn_return_type.is_option()
                            || self.current_fn_return_type.is_result() =>
                        {
                            self.current_fn_return_type.monomorphized_name()
                        }
                        None => {
                            // Same fallback as the tag-extraction site
                            // above. Keep in sync.
                            let fallback = match variant_name.as_str() {
                                "Ok" | "Err" => {
                                    Ty::Generic("Result".into(), vec![Ty::Int, Ty::String])
                                }
                                _ => Ty::Generic("Option".into(), vec![Ty::Int]),
                            };
                            self.register_adt_type(&fallback);
                            fallback.monomorphized_name()
                        }
                    };

                    // Extract payload from ADT and bind to the first field variable
                    // For Option: Some=field 1. For Result: Ok=field 1, Err=field 2.
                    let field_index = if variant_name == "Err" { 2 } else { 1 };
                    let payload = self.fresh_temp();
                    self.emit(
                        body,
                        Instruction::AdtPayload {
                            dest: payload.clone(),
                            obj: Operand::Var(subject.clone()),
                            type_name: subject_type_name.clone(),
                            field_index,
                        },
                    );

                    // Store into the pre-allocated alloca for this variable
                    let bind_name = &fields[0].field_name;
                    self.emit(
                        body,
                        Instruction::Store {
                            dest: bind_name.clone(),
                            value: Operand::Var(payload),
                        },
                    );

                    // Track the type of the bound variable. For Named inner
                    // types we register both `pattern_vars` (so Ident Load
                    // through the alloca stays ptr-typed) and `var_types`
                    // so downstream `resolve_struct_type` can locate the
                    // struct's field definitions for FieldGet lowering.
                    if let Some(subject_ty) = self.generic_var_types.get(&subject).cloned() {
                        let inner_ty = if variant_name == "Ok" {
                            subject_ty.result_ok_type().cloned()
                        } else if variant_name == "Err" {
                            subject_ty.result_err_type().cloned()
                        } else {
                            subject_ty.option_inner().cloned()
                        };
                        if let Some(inner) = inner_ty {
                            match &inner {
                                Ty::String => {
                                    self.string_vars.insert(bind_name.clone());
                                }
                                Ty::Float => {
                                    self.float_vars.insert(bind_name.clone());
                                }
                                Ty::Named(n) => {
                                    self.var_types.insert(bind_name.clone(), n.clone());
                                }
                                Ty::Generic(_, _) => {
                                    self.generic_var_types
                                        .insert(bind_name.clone(), inner.clone());
                                    self.var_types
                                        .insert(bind_name.clone(), inner.monomorphized_name());
                                }
                                _ => {}
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
                            // Use per-variant slot offset for correct field extraction.
                            let variant_offset = self
                                .variant_field_offsets
                                .get(&(stn.clone(), variant_name.clone()))
                                .copied()
                                .unwrap_or(1); // fallback: first payload slot
                            for (fi, pf) in fields.iter().enumerate() {
                                // §8.5: `when Card(last4)` is sugar for
                                // `when Card(last4: last4)`. The parser
                                // desugars so pf.field_name is the ADT
                                // field name; the actual binding name
                                // lives in pf.pattern (PatternKind::Ident)
                                // and may differ from the field name, e.g.
                                // `when Rectangle(width: w, height: h)`.
                                // Extract the binding name here so the
                                // Store target (and the string/float
                                // tracking maps) key on the user binding,
                                // not on the ADT field label.
                                let bind_name = match &pf.pattern.kind {
                                    tyra_ast::PatternKind::Ident(name) => name.clone(),
                                    tyra_ast::PatternKind::Wildcard => continue,
                                    _ => pf.field_name.clone(),
                                };
                                if bind_name == "_" {
                                    continue;
                                }
                                let field_index = (variant_offset + fi) as u32;
                                let payload = self.fresh_temp();
                                self.emit(
                                    body,
                                    Instruction::AdtPayload {
                                        dest: payload.clone(),
                                        obj: Operand::Var(subject.clone()),
                                        type_name: stn.clone(),
                                        field_index,
                                    },
                                );
                                // Store into the pre-allocated alloca for this variable
                                self.emit(
                                    body,
                                    Instruction::Store {
                                        dest: bind_name.clone(),
                                        value: Operand::Var(payload.clone()),
                                    },
                                );
                                // Track field type
                                if let Some((_, fty)) = vfields.get(fi) {
                                    match fty {
                                        Ty::String => {
                                            self.string_vars.insert(bind_name.clone());
                                        }
                                        Ty::Float => {
                                            self.float_vars.insert(bind_name.clone());
                                        }
                                        _ => {}
                                    }
                                }
                            }
                        }
                    }
                }
            }

            // Record where the user-written arm body starts (after all payload
            // binding instructions). block_ends_with_assignment must only see
            // instructions from the user body, not synthesised payload stores,
            // or it will mistake `Store { dest: "e__pN" }` for a user assignment
            // and skip storing the arm result into the match result slot.
            let arm_user_body_start = body.len();

            // Lower the arm body via the block-tail helper so the trailing
            // `Stmt::Expr` — including bare-Ident tails like `when Some(x) x`
            // — gets its value captured from `lower_expr`'s return rather
            // than from a scan of the emitted MIR. The helper also drops
            // the tail when it's a Unit-returning call (see
            // `is_unit_call_expr`) so the 066-square-list class of
            // void-recursive arms doesn't spill an undefined SSA value.
            let tail = self.lower_block_collect_tail(&arm.body, body);

            // If the arm body already ends with a block terminator (Return, Jump,
            // or BranchIf from a nested match/if), skip Store/Jump to avoid
            // emitting dead instructions after a terminator.
            let arm_terminates = super::range_terminates(body, arm_body_start);

            if !arm_terminates {
                // Store arm result into the alloca'd slot.
                // Skip when the arm's tail is a user assignment (`x = e`) — Tyra spec
                // makes that a Unit-typed statement, not the value of `e`.
                // Use arm_user_body_start (post-payload-binding) so synthesised
                // payload stores do not trigger this check.
                if !super::block_ends_with_assignment(body, arm_user_body_start) {
                    let last = match tail {
                        super::BlockTail::Value(v) => Some(v),
                        super::BlockTail::Unit => None,
                        super::BlockTail::Fallback => self.last_temp_in_range(body, arm_body_start),
                    };
                    if let Some(last) = last {
                        if self.string_vars.contains(&last) {
                            result_slot_is_string = true;
                        }
                        if self.float_vars.contains(&last) {
                            result_slot_is_float = true;
                        }
                        // Capture struct/ADT type from the arm's tail value
                        // so the match-result temp can be tracked downstream
                        // (`let a2 = match ... when Ok(acc) acc end` →
                        // a2 is Account, so `a2.balance` resolves).
                        if result_slot_var_type.is_none() {
                            if let Some(vt) = self.var_types.get(&last).cloned() {
                                result_slot_var_type = Some(vt);
                            }
                        }
                        if result_slot_generic_type.is_none() {
                            if let Some(gt) = self.generic_var_types.get(&last).cloned() {
                                result_slot_generic_type = Some(gt);
                            }
                        }
                        self.emit(
                            body,
                            Instruction::Store {
                                dest: result_slot.clone(),
                                value: Operand::Var(last),
                            },
                        );
                    }
                }

                self.emit_synthetic(
                    body,
                    Instruction::Jump {
                        label: end_label.clone(),
                    },
                );
            }

            // Next arm label
            if i + 1 < m.arms.len() {
                self.emit_synthetic(body, Instruction::Label(next_label.clone()));
            }
        }

        self.emit_synthetic(body, Instruction::Label(end_label));

        // Load the result from the alloca'd slot
        let result = self.fresh_temp();
        self.emit(
            body,
            Instruction::Load {
                dest: result.clone(),
                source: result_slot,
            },
        );
        if result_slot_is_string {
            self.string_vars.insert(result.clone());
        }
        if result_slot_is_float {
            self.float_vars.insert(result.clone());
        }
        if let Some(vt) = result_slot_var_type {
            self.var_types.insert(result.clone(), vt);
        }
        if let Some(gt) = result_slot_generic_type {
            self.generic_var_types.insert(result.clone(), gt);
        }
        result
    }

    /// Recursively pre-allocate all leaf Ident variables in a pattern.
    pub(super) fn pre_alloca_pattern_vars(
        &mut self,
        pattern: &PatternKind,
        body: &mut Vec<MirStmt>,
    ) {
        match pattern {
            PatternKind::Constructor(_, fields) => {
                for pf in fields {
                    match &pf.pattern.kind {
                        PatternKind::Ident(name) if name != "_" => {
                            if !self.mut_vars.contains(name) && !self.pattern_vars.contains(name) {
                                self.emit_synthetic(
                                    body,
                                    Instruction::Alloca { dest: name.clone() },
                                );
                                self.pattern_vars.insert(name.clone());
                                self.local_binding_names.insert(name.clone());
                            }
                        }
                        PatternKind::Constructor(_, _) => {
                            self.pre_alloca_pattern_vars(&pf.pattern.kind, body);
                        }
                        _ => {}
                    }
                }
            }
            _ => {}
        }
    }

    /// Recursively extract payload fields and bind pattern variables for a user-defined ADT.
    /// `subject` holds the extracted payload at this level.
    /// `type_name` / `variant_name` identify which ADT variant is being destructured.
    /// On inner tag mismatch, branches to `fail_label`.
    pub(super) fn lower_ctor_payload_and_vars(
        &mut self,
        subject: &str,
        type_name: &str,
        variant_name: &str,
        fields: &[PatternField],
        fail_label: &str,
        body: &mut Vec<MirStmt>,
    ) {
        let vfields: Vec<(String, Ty)> = self
            .adt_variant_fields
            .get(&(type_name.to_string(), variant_name.to_string()))
            .cloned()
            .unwrap_or_default();

        let variant_offset = self
            .variant_field_offsets
            .get(&(type_name.to_string(), variant_name.to_string()))
            .copied()
            .unwrap_or(1);

        for (fi, pf) in fields.iter().enumerate() {
            if pf.field_name == "_" {
                match &pf.pattern.kind {
                    PatternKind::Constructor(_, _) | PatternKind::Ident(_) => {}
                    _ => continue,
                }
            }

            // Use positional index: pf.field_name is the pattern variable name,
            // not necessarily the struct field name, so name-based lookup would be wrong.
            let field_index = (variant_offset + fi) as u32;

            let payload = self.fresh_temp();
            self.emit(
                body,
                Instruction::AdtPayload {
                    dest: payload.clone(),
                    obj: Operand::Var(subject.to_string()),
                    type_name: type_name.to_string(),
                    field_index,
                },
            );

            let field_ty = vfields.get(fi).map(|(_, ty)| ty.clone());

            match &pf.pattern.kind {
                PatternKind::Constructor(inner_variant, inner_fields) => {
                    // Determine inner ADT type name from field type
                    let inner_tn = field_ty.as_ref().and_then(|ty| match ty {
                        Ty::Named(n) => Some(n.clone()),
                        _ => None,
                    });
                    if let Some(inner_tn) = inner_tn {
                        if self.adt_struct_defs.contains_key(inner_tn.as_str()) {
                            // Check inner tag
                            let inner_tag = self
                                .variant_tags
                                .get(&(inner_tn.clone(), inner_variant.clone()))
                                .copied()
                                .unwrap_or(0);
                            let tag_val = self.fresh_temp();
                            self.emit(
                                body,
                                Instruction::AdtTag {
                                    dest: tag_val.clone(),
                                    obj: Operand::Var(payload.clone()),
                                    type_name: inner_tn.clone(),
                                },
                            );
                            let lit = self.fresh_temp();
                            self.emit(
                                body,
                                Instruction::Const {
                                    dest: lit.clone(),
                                    value: Constant::Int(inner_tag),
                                },
                            );
                            let cond = self.fresh_temp();
                            self.emit(
                                body,
                                Instruction::BinOp {
                                    dest: cond.clone(),
                                    op: MirBinOp::EqInt,
                                    lhs: Operand::Var(tag_val),
                                    rhs: Operand::Var(lit),
                                },
                            );
                            let ok_label = self.fresh_label("nested_ok");
                            self.emit_synthetic(
                                body,
                                Instruction::BranchIf {
                                    cond: Operand::Var(cond),
                                    true_label: ok_label.clone(),
                                    false_label: fail_label.to_string(),
                                },
                            );
                            self.emit_synthetic(body, Instruction::Label(ok_label));
                            // Recurse
                            self.lower_ctor_payload_and_vars(
                                &payload,
                                &inner_tn,
                                inner_variant,
                                inner_fields,
                                fail_label,
                                body,
                            );
                        }
                    }
                }
                PatternKind::Ident(var_name) if var_name != "_" => {
                    self.emit(
                        body,
                        Instruction::Store {
                            dest: var_name.clone(),
                            value: Operand::Var(payload),
                        },
                    );
                    if let Some(ty) = field_ty {
                        match ty {
                            Ty::String => {
                                self.string_vars.insert(var_name.clone());
                            }
                            Ty::Float => {
                                self.float_vars.insert(var_name.clone());
                            }
                            _ => {}
                        }
                    }
                }
                _ => {}
            }
        }
    }
}
