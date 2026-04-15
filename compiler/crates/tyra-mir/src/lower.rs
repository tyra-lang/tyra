// AST to MIR lowering.
//
// Walks the AST and produces a flat sequence of MIR instructions.
// Expressions are flattened into named temporaries.
// Control flow is desugared into labels and branches.

use tyra_ast::*;
use tyra_types::Ty;

use crate::ir::*;

/// Lower a source file to MIR.
pub fn lower(file: &SourceFile) -> Program {
    let mut ctx = LowerCtx::new();

    let has_explicit_main = file
        .items
        .iter()
        .any(|item| matches!(item, Item::FnDef(f) if f.name == "main"));

    let has_top_level_stmts = file.items.iter().any(|item| matches!(item, Item::Stmt(_)));

    // ADR-0006 Rule 2: fn main and top-level statements are mutually exclusive.
    // This should already be caught by the parser/resolver, but we enforce it here
    // defensively to avoid producing invalid MIR with duplicate main functions.
    assert!(
        !(has_explicit_main && has_top_level_stmts),
        "BUG: fn main and top-level statements both present (ADR-0006 Rule 2 violation)"
    );

    // Lower function definitions
    for item in &file.items {
        if let Item::FnDef(f) = item {
            let mut func = ctx.lower_fn(f);
            if f.name == "main" {
                func.is_main = true;
            }
            ctx.functions.push(func);
        }
    }

    // Lower top-level statements into an implicit main (§6.1)
    if has_top_level_stmts {
        let mut body = Vec::new();
        for item in &file.items {
            if let Item::Stmt(s) = item {
                ctx.lower_stmt(s, &mut body);
            }
        }
        body.push(Instruction::Return { value: None });

        ctx.functions.push(Function {
            name: "main".into(),
            params: vec![],
            return_type: Ty::Unit,
            body,
            is_main: true,
        });
    }

    Program {
        functions: ctx.functions,
        string_constants: ctx.string_constants,
    }
}

struct LowerCtx {
    functions: Vec<Function>,
    string_constants: Vec<String>,
    temp_counter: u32,
    label_counter: u32,
}

impl LowerCtx {
    fn new() -> Self {
        Self {
            functions: Vec::new(),
            string_constants: Vec::new(),
            temp_counter: 0,
            label_counter: 0,
        }
    }

    fn fresh_temp(&mut self) -> String {
        let t = format!("_t{}", self.temp_counter);
        self.temp_counter += 1;
        t
    }

    fn fresh_label(&mut self, prefix: &str) -> String {
        let l = format!("{prefix}_{}", self.label_counter);
        self.label_counter += 1;
        l
    }

    fn intern_string(&mut self, s: &str) -> usize {
        if let Some(idx) = self.string_constants.iter().position(|c| c == s) {
            idx
        } else {
            let idx = self.string_constants.len();
            self.string_constants.push(s.to_string());
            idx
        }
    }

    fn lower_fn(&mut self, f: &FnDef) -> Function {
        let params: Vec<(String, Ty)> = f
            .params
            .iter()
            .map(|p| (p.name.clone(), Ty::from_type_expr(&p.type_annotation)))
            .collect();
        let return_type = f
            .return_type
            .as_ref()
            .map(Ty::from_type_expr)
            .unwrap_or(Ty::Unit);

        let mut body = Vec::new();
        for stmt in &f.body {
            self.lower_stmt(stmt, &mut body);
        }

        // If last instruction isn't a return, add implicit return
        if !matches!(body.last(), Some(Instruction::Return { .. })) {
            if return_type == Ty::Unit {
                body.push(Instruction::Return { value: None });
            } else if let Some(last_temp) = self.last_temp_name(&body) {
                body.push(Instruction::Return {
                    value: Some(Operand::Var(last_temp)),
                });
            } else {
                body.push(Instruction::Return { value: None });
            }
        }

        Function {
            name: f.name.clone(),
            params,
            return_type,
            body,
            is_main: false,
        }
    }

    fn lower_stmt(&mut self, stmt: &Stmt, body: &mut Vec<Instruction>) {
        match stmt {
            Stmt::Let(s) => {
                let val = self.lower_expr(&s.value, body);
                body.push(Instruction::Copy {
                    dest: s.name.clone(),
                    source: val,
                });
            }
            Stmt::Mut(s) => {
                let val = self.lower_expr(&s.value, body);
                body.push(Instruction::Copy {
                    dest: s.name.clone(),
                    source: val,
                });
            }
            Stmt::Return(s) => {
                let value = s.value.as_ref().map(|v| {
                    let t = self.lower_expr(v, body);
                    Operand::Var(t)
                });
                body.push(Instruction::Return { value });
            }
            Stmt::Defer(_) => {
                // defer lowering: deferred to later milestone
                // For now, the deferred expression is simply ignored in MIR
            }
            Stmt::Expr(s) => {
                self.lower_expr(&s.expr, body);
            }
        }
    }

    /// Lower an expression, returning the name of the temporary holding the result.
    fn lower_expr(&mut self, expr: &Expr, body: &mut Vec<Instruction>) -> String {
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

            ExprKind::Ident(name) => name.clone(),

            ExprKind::BinaryOp(lhs, op, rhs) => {
                let l = self.lower_expr(lhs, body);
                let r = self.lower_expr(rhs, body);
                let dest = self.fresh_temp();
                let is_float = is_float_expr(lhs) || is_float_expr(rhs);
                let mir_op = ast_binop_to_mir(*op, is_float);
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
                    func: func_name,
                    args: arg_operands,
                });
                dest
            }

            ExprKind::Assign(lhs, rhs) => {
                let val = self.lower_expr(rhs, body);
                if let ExprKind::Ident(name) = &lhs.kind {
                    body.push(Instruction::Copy {
                        dest: name.clone(),
                        source: val.clone(),
                    });
                }
                val
            }

            ExprKind::If(if_expr) => self.lower_if(if_expr, body),

            ExprKind::Match(m) => {
                // TODO: Proper match lowering with pattern dispatch.
                // Current implementation only lowers the subject and first arm body.
                // Full pattern matching requires: condition generation per pattern,
                // branch dispatch, and exhaustiveness is already checked by tyra-types.
                self.lower_expr(&m.subject, body);
                let result = self.fresh_temp();
                if let Some(first_arm) = m.arms.first() {
                    for stmt in &first_arm.body {
                        self.lower_stmt(stmt, body);
                    }
                }
                body.push(Instruction::Const {
                    dest: result.clone(),
                    value: Constant::Unit,
                });
                result
            }

            ExprKind::For(f) => {
                let iter_val = self.lower_expr(&f.iter, body);
                // Simplified: lower body once (no actual iteration in MIR yet)
                body.push(Instruction::Copy {
                    dest: f.binding.clone(),
                    source: iter_val,
                });
                for stmt in &f.body {
                    self.lower_stmt(stmt, body);
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
                // ? operator: simplified, just lower the inner expression
                self.lower_expr(inner, body)
            }

            ExprKind::Await(inner) => {
                // .await: simplified, just lower the inner expression
                self.lower_expr(inner, body)
            }

            ExprKind::Spawn(inner) => self.lower_expr(inner, body),

            ExprKind::FieldAccess(obj, field) => {
                let obj_val = self.lower_expr(obj, body);
                let dest = self.fresh_temp();
                body.push(Instruction::Copy {
                    dest: dest.clone(),
                    source: format!("{obj_val}.{field}"),
                });
                dest
            }

            ExprKind::Index(obj, idx) => {
                self.lower_expr(obj, body);
                self.lower_expr(idx, body);
                let dest = self.fresh_temp();
                body.push(Instruction::Const {
                    dest: dest.clone(),
                    value: Constant::Unit,
                });
                dest
            }

            ExprKind::Lambda(_) | ExprKind::TurbofishCall(_, _, _) => {
                let dest = self.fresh_temp();
                body.push(Instruction::Const {
                    dest: dest.clone(),
                    value: Constant::Unit,
                });
                dest
            }

            ExprKind::ListLit(items) => {
                for item in items {
                    self.lower_expr(item, body);
                }
                let dest = self.fresh_temp();
                body.push(Instruction::Const {
                    dest: dest.clone(),
                    value: Constant::Unit,
                });
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
                // TODO: String interpolation requires runtime string concatenation.
                // Current implementation only includes literal parts and evaluates
                // (but discards) interpolated expressions. Full implementation needs
                // a runtime concat/format function.
                let mut combined = String::new();
                for part in parts {
                    match part {
                        StringPart::Lit(s) => combined.push_str(s),
                        StringPart::Expr(e) => {
                            // Evaluate for side effects, but result is discarded
                            self.lower_expr(e, body);
                        }
                    }
                }
                let idx = self.intern_string(&combined);
                let dest = self.fresh_temp();
                body.push(Instruction::Const {
                    dest: dest.clone(),
                    value: Constant::StringRef(idx),
                });
                dest
            }
        }
    }

    fn lower_if(&mut self, if_expr: &IfExpr, body: &mut Vec<Instruction>) -> String {
        let cond = self.lower_expr(&if_expr.condition, body);
        let then_label = self.fresh_label("then");
        let else_label = self.fresh_label("else");
        let end_label = self.fresh_label("if_end");

        body.push(Instruction::BranchIf {
            cond: Operand::Var(cond),
            true_label: then_label.clone(),
            false_label: else_label.clone(),
        });

        // Then branch
        body.push(Instruction::Label(then_label));
        for stmt in &if_expr.then_body {
            self.lower_stmt(stmt, body);
        }
        body.push(Instruction::Jump {
            label: end_label.clone(),
        });

        // Else branch
        body.push(Instruction::Label(else_label));
        match &if_expr.else_body {
            Some(ElseBranch::Else(stmts)) => {
                for stmt in stmts {
                    self.lower_stmt(stmt, body);
                }
            }
            Some(ElseBranch::ElseIf(inner)) => {
                self.lower_if(inner, body);
            }
            None => {}
        }
        body.push(Instruction::Jump {
            label: end_label.clone(),
        });

        body.push(Instruction::Label(end_label));

        let dest = self.fresh_temp();
        body.push(Instruction::Const {
            dest: dest.clone(),
            value: Constant::Unit,
        });
        dest
    }

    fn last_temp_name(&self, body: &[Instruction]) -> Option<String> {
        for inst in body.iter().rev() {
            match inst {
                Instruction::Const { dest, .. }
                | Instruction::Call {
                    dest: Some(dest), ..
                }
                | Instruction::BinOp { dest, .. }
                | Instruction::Neg { dest, .. }
                | Instruction::Not { dest, .. }
                | Instruction::Copy { dest, .. } => return Some(dest.clone()),
                _ => continue,
            }
        }
        None
    }
}

/// Check if an expression is a float literal (for MIR op selection).
fn is_float_expr(expr: &Expr) -> bool {
    matches!(expr.kind, ExprKind::FloatLit(_))
}

/// Convert AST binary op to MIR op, selecting Int or Float variant.
fn ast_binop_to_mir(op: BinOp, is_float: bool) -> MirBinOp {
    match (op, is_float) {
        (BinOp::Add, false) => MirBinOp::AddInt,
        (BinOp::Add, true) => MirBinOp::AddFloat,
        (BinOp::Sub, false) => MirBinOp::SubInt,
        (BinOp::Sub, true) => MirBinOp::SubFloat,
        (BinOp::Mul, false) => MirBinOp::MulInt,
        (BinOp::Mul, true) => MirBinOp::MulFloat,
        (BinOp::Div, false) => MirBinOp::DivInt,
        (BinOp::Div, true) => MirBinOp::DivFloat,
        (BinOp::Eq, _) => MirBinOp::EqInt,
        (BinOp::NotEq, _) => MirBinOp::NeqInt,
        (BinOp::Lt, false) => MirBinOp::LtInt,
        (BinOp::Lt, true) => MirBinOp::LtFloat,
        (BinOp::LtEq, false) => MirBinOp::LeInt,
        (BinOp::LtEq, true) => MirBinOp::LeFloat,
        (BinOp::Gt, false) => MirBinOp::GtInt,
        (BinOp::Gt, true) => MirBinOp::GtFloat,
        (BinOp::GtEq, false) => MirBinOp::GeInt,
        (BinOp::GtEq, true) => MirBinOp::GeFloat,
        (BinOp::RefEq, _) => MirBinOp::EqInt,
        (BinOp::And, _) => MirBinOp::And,
        (BinOp::Or, _) => MirBinOp::Or,
    }
}
