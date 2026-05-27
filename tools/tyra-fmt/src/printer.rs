// AST-to-source printer for the Tyra formatter.
//
// Design:
// - Comments are extracted by line number and re-injected before the node they precede.
// - Blank lines between top-level items are normalised to exactly one.
// - All expressions are formatted inline (no line-wrapping within expressions in v0.2).
// - Indentation: 2 spaces per level.

use std::collections::BTreeMap;
use tyra_ast::*;
use tyra_diagnostics::{SourceId, SourceMap, Span};

// ─── Public surface ──────────────────────────────────────────────────────────

/// Scan `src` and return two maps keyed by 1-based line number:
///   - `standalone`: lines whose first non-whitespace character is `#`
///   - `inline`: lines that have code before a `#` comment (e.g. `let x = 1 # note`)
///
/// String literals are handled so `"#{x}"` interpolation is not misidentified.
pub fn extract_comments(src: &str) -> (BTreeMap<u32, String>, BTreeMap<u32, String>) {
    let mut standalone = BTreeMap::new();
    let mut inline = BTreeMap::new();
    for (i, line) in src.lines().enumerate() {
        let line_no = (i + 1) as u32;
        let trimmed = line.trim_start();
        if trimmed.starts_with('#') {
            standalone.insert(line_no, trimmed.to_string());
        } else if let Some(comment) = find_inline_comment(line) {
            inline.insert(line_no, comment.to_string());
        }
    }
    (standalone, inline)
}

/// Find an inline comment on `line` (a `#` that appears outside string
/// literals and after at least one non-whitespace character).  Returns the
/// comment text including the leading `#`.
fn find_inline_comment(line: &str) -> Option<&str> {
    let mut in_string = false;
    let mut escape_next = false;
    let bytes = line.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        let b = bytes[i];
        if escape_next {
            escape_next = false;
        } else if in_string {
            match b {
                b'\\' => escape_next = true,
                b'"' => in_string = false,
                _ => {}
            }
        } else {
            match b {
                b'"' => in_string = true,
                b'#' => {
                    let before = line[..i].trim();
                    if !before.is_empty() {
                        return Some(line[i..].trim_end());
                    }
                    return None;
                }
                _ => {}
            }
        }
        i += 1;
    }
    None
}

pub struct Printer<'src> {
    _src: &'src str,
    sid: SourceId,
    sources: &'src SourceMap,
    comments: BTreeMap<u32, String>,
    inline_comments: BTreeMap<u32, String>,
    out: String,
    indent: usize,
    /// The last source line whose content we have already "consumed"
    /// (either as a comment or as a code node).  Comment scanning uses
    /// `last_line + 1` as the lower bound so comments are not re-emitted.
    last_line: u32,
}

impl<'src> Printer<'src> {
    pub fn new(
        src: &'src str,
        sid: SourceId,
        sources: &'src SourceMap,
        comments: BTreeMap<u32, String>,
        inline_comments: BTreeMap<u32, String>,
    ) -> Self {
        Self {
            _src: src,
            sid,
            sources,
            comments,
            inline_comments,
            out: String::new(),
            indent: 0,
            last_line: 0,
        }
    }

    // ── Utilities ────────────────────────────────────────────────────────────

    fn indent_str(&self) -> String {
        "  ".repeat(self.indent)
    }

    /// Length of the current (last) line in `self.out` (characters after the
    /// last `\n`).  Used to decide whether a parameter list needs wrapping.
    fn current_col(&self) -> usize {
        match self.out.rfind('\n') {
            Some(pos) => self.out.len() - pos - 1,
            None => self.out.len(),
        }
    }

    fn line_of(&self, offset: u32) -> u32 {
        self.sources.line_col(self.sid, offset).0
    }

    /// Append an inline comment (code-trailing `# ...`) for `line` if one exists.
    fn emit_inline_comment_if_any(&mut self, line: u32) {
        if let Some(comment) = self.inline_comments.get(&line).cloned() {
            self.out.push(' ');
            self.out.push_str(&comment);
        }
    }

    /// Emit all comments whose line number falls in `(self.last_line, before_line)`.
    /// Updates `self.last_line` to `before_line - 1`.
    fn emit_pending_comments(&mut self, before_line: u32) {
        if before_line == 0 {
            return;
        }
        let indent = self.indent_str();
        if self.last_line + 1 >= before_line {
            return;
        }
        let lines: Vec<String> = self
            .comments
            .range(self.last_line + 1..before_line)
            .map(|(_, v)| v.clone())
            .collect();
        for text in lines {
            self.out.push_str(&indent);
            self.out.push_str(&text);
            self.out.push('\n');
        }
        if before_line > 0 {
            self.last_line = before_line - 1;
        }
    }

    // ── Entry point ──────────────────────────────────────────────────────────

    pub fn print_file(mut self, ast: &SourceFile) -> String {
        // Do NOT early-return on empty items: file may contain only comments.
        let mut first = true;
        for item in &ast.items {
            let item_start_line = self.line_of(item_span(item).start);

            // Collect comments that precede this item.
            // Guard: only call range() when the range is non-empty and valid.
            let comments_before: Vec<String> = if self.last_line + 1 < item_start_line {
                self.comments
                    .range(self.last_line + 1..item_start_line)
                    .map(|(_, v)| v.clone())
                    .collect()
            } else {
                vec![]
            };

            // One blank line between items.
            if !first {
                self.out.push('\n');
            }

            // Emit preceding comments at top-level (indent = 0).
            for comment in &comments_before {
                self.out.push_str(comment);
                self.out.push('\n');
            }

            self.last_line = item_start_line.saturating_sub(1);
            self.print_item(item);
            // Import and top-level Stmt have no internal \n after their content;
            // other item types (fn, value, data, trait, impl) handle the header-line
            // inline comment inside their own printers before the header \n.
            match item {
                Item::Import(_) | Item::Stmt(_) => {
                    self.emit_inline_comment_if_any(item_start_line);
                }
                _ => {}
            }
            self.out.push('\n');

            // Use saturating_sub(1) for consistency with print_stmts span handling.
            let item_end_line = self.line_of(item_span(item).end.saturating_sub(1));
            self.last_line = self.last_line.max(item_end_line);

            first = false;
        }

        // Flush standalone comments that appear after all items (or all comments
        // when the file has no items at all, e.g. a comment-only file).
        let trailing: Vec<String> = self
            .comments
            .range(self.last_line + 1..)
            .map(|(_, v)| v.clone())
            .collect();
        for comment in trailing {
            self.out.push_str(&comment);
            self.out.push('\n');
        }

        self.out
    }

    // ── Items ─────────────────────────────────────────────────────────────────

    fn print_item(&mut self, item: &Item) {
        match item {
            Item::FnDef(f) => self.print_fn(f),
            Item::ValueDef(v) => self.print_value_def(v),
            Item::DataDef(d) => self.print_data_def(d),
            Item::TypeDef(t) => self.print_type_def(t),
            Item::TraitDef(t) => self.print_trait_def(t),
            Item::ImplDef(i) => self.print_impl_def(i),
            Item::Import(i) => self.print_import(i),
            Item::Stmt(s) => self.print_stmt(s),
            Item::TestDef(td) => self.print_test_def(td),
        }
    }

    fn print_test_def(&mut self, td: &TestDef) {
        let indent = self.indent_str();
        let modifier = if td.expects_panic { " panics" } else { "" };
        self.out
            .push_str(&format!("{indent}test {:?}{modifier}\n", td.name));
        self.indent += 1;
        for stmt in &td.body {
            self.print_stmt(stmt);
        }
        self.indent -= 1;
        self.out.push_str(&format!("{indent}end\n"));
    }

    fn print_fn(&mut self, f: &FnDef) {
        let indent = self.indent_str();
        let fn_line = self.line_of(f.span.start);
        self.out.push_str(&indent);
        self.print_fn_header(f);
        self.emit_inline_comment_if_any(fn_line);
        self.out.push('\n');

        self.last_line = self.last_line.max(fn_line);
        self.indent += 1;
        self.print_stmts(&f.body);
        self.indent -= 1;

        let indent = self.indent_str();
        self.out.push_str(&indent);
        self.out.push_str("end");
    }

    fn print_value_def(&mut self, v: &ValueDef) {
        let indent = self.indent_str();
        let v_line = self.line_of(v.span.start);
        self.out.push_str(&indent);
        if v.is_export {
            self.out.push_str("export ");
        }
        self.out.push_str("value ");
        self.out.push_str(&v.name);
        self.print_type_params(&v.type_params);
        self.emit_inline_comment_if_any(v_line);
        self.out.push('\n');
        self.indent += 1;
        for field in &v.fields {
            let ind = self.indent_str();
            self.out.push_str(&ind);
            if field.is_mut {
                self.out.push_str("mut ");
            }
            self.out.push_str(&field.name);
            self.out.push_str(": ");
            self.print_type_expr(&field.type_annotation);
            self.out.push('\n');
        }
        self.indent -= 1;
        let indent = self.indent_str();
        self.out.push_str(&indent);
        self.out.push_str("end");
    }

    fn print_data_def(&mut self, d: &DataDef) {
        let indent = self.indent_str();
        let d_line = self.line_of(d.span.start);
        self.out.push_str(&indent);
        if d.is_export {
            self.out.push_str("export ");
        }
        self.out.push_str("data ");
        self.out.push_str(&d.name);
        self.print_type_params(&d.type_params);
        self.emit_inline_comment_if_any(d_line);
        self.out.push('\n');
        self.indent += 1;
        for field in &d.fields {
            let ind = self.indent_str();
            self.out.push_str(&ind);
            if field.is_mut {
                self.out.push_str("mut ");
            }
            self.out.push_str(&field.name);
            self.out.push_str(": ");
            self.print_type_expr(&field.type_annotation);
            self.out.push('\n');
        }
        self.indent -= 1;
        let indent = self.indent_str();
        self.out.push_str(&indent);
        self.out.push_str("end");
    }

    fn print_type_def(&mut self, t: &TypeDef) {
        let t_line = self.line_of(t.span.start);
        let indent = self.indent_str();
        self.out.push_str(&indent);
        if t.is_export {
            self.out.push_str("export ");
        }
        self.out.push_str("type ");
        self.out.push_str(&t.name);
        self.print_type_params(&t.type_params);
        match &t.kind {
            TypeDefKind::Alias(ty) => {
                self.out.push_str(" = ");
                self.print_type_expr(ty);
                self.emit_inline_comment_if_any(t_line);
            }
            TypeDefKind::Adt(variants) => {
                self.out.push_str(" =");
                self.emit_inline_comment_if_any(t_line);
                let var_indent = "  ".repeat(self.indent + 1);
                for variant in variants {
                    self.out.push('\n');
                    self.out.push_str(&var_indent);
                    self.out.push_str("| ");
                    self.out.push_str(&variant.name);
                    if !variant.fields.is_empty() {
                        self.out.push('(');
                        let mut first = true;
                        for field in &variant.fields {
                            if !first {
                                self.out.push_str(", ");
                            }
                            self.out.push_str(&field.name);
                            self.out.push_str(": ");
                            self.print_type_expr(&field.type_annotation);
                            first = false;
                        }
                        self.out.push(')');
                    }
                }
            }
        }
    }

    fn print_trait_def(&mut self, t: &TraitDef) {
        let indent = self.indent_str();
        let t_line = self.line_of(t.span.start);
        self.out.push_str(&indent);
        if t.is_export {
            self.out.push_str("export ");
        }
        self.out.push_str("trait ");
        self.out.push_str(&t.name);
        self.print_type_params(&t.type_params);
        self.emit_inline_comment_if_any(t_line);
        self.out.push('\n');
        self.indent += 1;
        for method in &t.methods {
            let m_line = self.line_of(method.span.start);
            let ind = self.indent_str();
            self.out.push_str(&ind);
            self.print_fn_header(method);
            self.emit_inline_comment_if_any(m_line);
            if !method.body.is_empty() {
                self.out.push('\n');
                self.last_line = self.last_line.max(m_line);
                self.indent += 1;
                self.print_stmts(&method.body);
                self.indent -= 1;
                let ind2 = self.indent_str();
                self.out.push_str(&ind2);
                self.out.push_str("end");
            }
            self.out.push('\n');
        }
        self.indent -= 1;
        let indent = self.indent_str();
        self.out.push_str(&indent);
        self.out.push_str("end");
    }

    fn print_impl_def(&mut self, i: &ImplDef) {
        let indent = self.indent_str();
        let impl_line = self.line_of(i.span.start);
        self.out.push_str(&indent);
        self.out.push_str("impl ");
        self.out.push_str(&i.trait_name);
        if !i.trait_type_args.is_empty() {
            self.out.push('<');
            let mut first = true;
            for ty in &i.trait_type_args {
                if !first {
                    self.out.push_str(", ");
                }
                self.print_type_expr(ty);
                first = false;
            }
            self.out.push('>');
        }
        self.out.push_str(" for ");
        self.print_type_expr(&i.target_type);
        self.emit_inline_comment_if_any(impl_line);
        self.out.push('\n');
        self.indent += 1;
        for method in &i.methods {
            self.print_fn(method);
            self.out.push('\n');
        }
        self.indent -= 1;
        let indent = self.indent_str();
        self.out.push_str(&indent);
        self.out.push_str("end");
    }

    fn print_import(&mut self, i: &ImportDecl) {
        let indent = self.indent_str();
        self.out.push_str(&indent);
        self.out.push_str("import ");
        self.out.push_str(&i.path.join("."));
        if let Some(alias) = &i.alias {
            self.out.push_str(" as ");
            self.out.push_str(alias);
        }
    }

    // ── Helpers shared between fn, trait, impl ────────────────────────────────

    fn print_fn_header(&mut self, f: &FnDef) {
        // Build prefix up to the opening paren.
        let mut prefix = String::new();
        if f.is_export {
            prefix.push_str("export ");
        }
        if f.is_async {
            prefix.push_str("async ");
        }
        prefix.push_str("fn ");
        prefix.push_str(&f.name);

        // Collect type-param text (rarely long; reuse existing method via a scratch printer).
        let tp_text = {
            let saved = std::mem::take(&mut self.out);
            self.print_type_params(&f.type_params);
            std::mem::replace(&mut self.out, saved)
        };
        prefix.push_str(&tp_text);

        // Render each parameter to a string.
        let render_param = |printer: &mut Self, p: &Param| -> String {
            let saved = std::mem::take(&mut printer.out);
            printer.print_param(p);
            std::mem::replace(&mut printer.out, saved)
        };
        let mut param_strs: Vec<String> = Vec::new();
        if f.self_param.is_some() {
            param_strs.push("self".to_string());
        }
        for param in &f.params {
            param_strs.push(render_param(self, param));
        }

        // Render return type.
        let ret_text: Option<String> = f.return_type.as_ref().map(|ret| {
            let saved = std::mem::take(&mut self.out);
            self.print_type_expr(ret);
            std::mem::replace(&mut self.out, saved)
        });

        // Try single-line form first.
        let inline_params = param_strs.join(", ");
        let mut single = format!("{prefix}({inline_params})");
        if let Some(r) = &ret_text {
            single.push_str(" -> ");
            single.push_str(r);
        }

        const LINE_LIMIT: usize = 100;
        let col_before = self.current_col();
        if col_before + single.len() <= LINE_LIMIT || param_strs.is_empty() {
            self.out.push_str(&single);
        } else {
            // Multi-line form: one parameter per line, indented by 4 spaces relative
            // to the current indent level (aligns with the opening paren position).
            let cont_indent = format!("{}    ", self.indent_str());
            self.out.push_str(&prefix);
            self.out.push('(');
            for (i, ps) in param_strs.iter().enumerate() {
                self.out.push('\n');
                self.out.push_str(&cont_indent);
                self.out.push_str(ps);
                if i + 1 < param_strs.len() {
                    self.out.push(',');
                }
            }
            self.out.push('\n');
            self.out.push_str(&self.indent_str());
            self.out.push(')');
            if let Some(r) = &ret_text {
                self.out.push_str(" -> ");
                self.out.push_str(r);
            }
        }
    }

    fn print_type_params(&mut self, tps: &[TypeParam]) {
        if tps.is_empty() {
            return;
        }
        self.out.push('<');
        let mut first = true;
        for tp in tps {
            if !first {
                self.out.push_str(", ");
            }
            self.out.push_str(&tp.name);
            if !tp.constraints.is_empty() {
                self.out.push_str(": ");
                let mut cf = true;
                for c in &tp.constraints {
                    if !cf {
                        self.out.push_str(" + ");
                    }
                    self.print_type_expr(c);
                    cf = false;
                }
            }
            first = false;
        }
        self.out.push('>');
    }

    fn print_param(&mut self, p: &Param) {
        match &p.label {
            None => {
                self.out.push_str("_ ");
                self.out.push_str(&p.name);
            }
            Some(label) if label == &p.name => {
                self.out.push_str(label);
            }
            Some(label) => {
                self.out.push_str(label);
                self.out.push(' ');
                self.out.push_str(&p.name);
            }
        }
        self.out.push_str(": ");
        self.print_type_expr(&p.type_annotation);
    }

    fn print_type_expr(&mut self, ty: &TypeExpr) {
        match &ty.kind {
            TypeExprKind::Named(n) => self.out.push_str(n),
            TypeExprKind::Generic(n, args) => {
                self.out.push_str(n);
                self.out.push('<');
                let mut first = true;
                for a in args {
                    if !first {
                        self.out.push_str(", ");
                    }
                    self.print_type_expr(a);
                    first = false;
                }
                self.out.push('>');
            }
            TypeExprKind::Fn(params, ret) => {
                self.out.push_str("fn(");
                let mut first = true;
                for p in params {
                    if !first {
                        self.out.push_str(", ");
                    }
                    self.print_type_expr(p);
                    first = false;
                }
                self.out.push_str(") -> ");
                self.print_type_expr(ret);
            }
        }
    }

    // ── Statements ────────────────────────────────────────────────────────────

    fn print_stmts(&mut self, stmts: &[Stmt]) {
        for stmt in stmts {
            let stmt_start_line = self.line_of(stmt_span(stmt).start);
            // Use end-1 to avoid the "next-line leakage" caused by call/index
            // expressions where parse_postfix records ts.peek_span() (the
            // Newline token) as the span end, making span.end point to the
            // first byte of the next line rather than the last byte of this one.
            let stmt_end_line = self.line_of(stmt_span(stmt).end.saturating_sub(1));
            self.emit_pending_comments(stmt_start_line);
            self.print_stmt(stmt);
            // Re-emit any inline comment that was on the same source line.
            if let Some(comment) = self.inline_comments.get(&stmt_start_line).cloned() {
                self.out.push(' ');
                self.out.push_str(&comment);
            }
            self.out.push('\n');
            self.last_line = self.last_line.max(stmt_end_line);
        }
    }

    fn print_stmt(&mut self, stmt: &Stmt) {
        let indent = self.indent_str();
        match stmt {
            Stmt::Let(s) => {
                self.out.push_str(&indent);
                self.out.push_str("let ");
                self.out.push_str(&s.name);
                if let Some(ty) = &s.type_annotation {
                    self.out.push_str(": ");
                    self.print_type_expr(ty);
                }
                self.out.push_str(" = ");
                self.print_expr(&s.value);
            }
            Stmt::Mut(s) => {
                self.out.push_str(&indent);
                self.out.push_str("mut ");
                self.out.push_str(&s.name);
                if let Some(ty) = &s.type_annotation {
                    self.out.push_str(": ");
                    self.print_type_expr(ty);
                }
                self.out.push_str(" = ");
                self.print_expr(&s.value);
            }
            Stmt::Return(s) => {
                self.out.push_str(&indent);
                self.out.push_str("return");
                if let Some(v) = &s.value {
                    self.out.push(' ');
                    self.print_expr(v);
                }
            }
            Stmt::Defer(s) => {
                self.out.push_str(&indent);
                self.out.push_str("defer ");
                self.print_expr(&s.expr);
            }
            Stmt::Break(_) => {
                self.out.push_str(&indent);
                self.out.push_str("break");
            }
            Stmt::Continue(_) => {
                self.out.push_str(&indent);
                self.out.push_str("continue");
            }
            Stmt::Expr(s) => {
                self.out.push_str(&indent);
                self.print_expr(&s.expr);
            }
        }
    }

    // ── Expressions ───────────────────────────────────────────────────────────

    fn print_expr(&mut self, expr: &Expr) {
        match &expr.kind {
            ExprKind::IntLit(n) => self.out.push_str(&n.to_string()),
            ExprKind::FloatLit(f) => {
                let s = if f.is_finite() && f.fract() == 0.0 {
                    format!("{:.1}", f)
                } else {
                    format!("{}", f)
                };
                self.out.push_str(&s);
            }
            ExprKind::StringLit(s) => {
                self.out.push('"');
                self.out.push_str(&escape_string(s));
                self.out.push('"');
            }
            ExprKind::StringInterp(parts) => {
                self.out.push('"');
                for part in parts {
                    match part {
                        StringPart::Lit(s) => self.out.push_str(&escape_string(s)),
                        StringPart::Expr(e) => {
                            self.out.push_str("#{");
                            self.print_expr(e);
                            self.out.push('}');
                        }
                    }
                }
                self.out.push('"');
            }
            ExprKind::BoolLit(b) => self.out.push_str(if *b { "true" } else { "false" }),
            ExprKind::UnitLit => self.out.push_str("()"),
            ExprKind::ListLit(elems) => {
                self.out.push('[');
                let mut first = true;
                for e in elems {
                    if !first {
                        self.out.push_str(", ");
                    }
                    self.print_expr(e);
                    first = false;
                }
                self.out.push(']');
            }
            ExprKind::MapLit(pairs) => {
                self.out.push('{');
                let mut first = true;
                for (k, v) in pairs {
                    if !first {
                        self.out.push_str(", ");
                    }
                    self.print_expr(k);
                    self.out.push_str(": ");
                    self.print_expr(v);
                    first = false;
                }
                self.out.push('}');
            }
            ExprKind::Ident(s) => self.out.push_str(s),
            ExprKind::FieldAccess(e, field) => {
                self.print_expr_prec(e, PREC_POSTFIX);
                self.out.push('.');
                self.out.push_str(field);
            }
            ExprKind::BinaryOp(lhs, op, rhs) => {
                let my_prec = bin_op_prec(*op);
                // Wrap lhs only if it has strictly lower binding (higher prec number).
                self.print_expr_prec(lhs, my_prec - 1);
                self.out.push(' ');
                self.out.push_str(bin_op_str(*op));
                self.out.push(' ');
                // Wrap rhs if same or lower binding to enforce left-associativity.
                self.print_expr_prec(rhs, my_prec);
            }
            ExprKind::UnaryOp(op, e) => match op {
                UnaryOp::Neg => {
                    self.out.push('-');
                    self.print_expr_prec(e, PREC_UNARY);
                }
                UnaryOp::Not => {
                    self.out.push_str("not ");
                    self.print_expr_prec(e, PREC_UNARY);
                }
            },
            ExprKind::Assign(lhs, rhs) => {
                self.print_expr(lhs);
                self.out.push_str(" = ");
                self.print_expr(rhs);
            }
            ExprKind::Call(callee, args) => {
                self.print_expr_prec(callee, PREC_POSTFIX);
                self.out.push('(');
                let mut first = true;
                for arg in args {
                    if !first {
                        self.out.push_str(", ");
                    }
                    self.print_arg(arg);
                    first = false;
                }
                self.out.push(')');
            }
            ExprKind::TurbofishCall(callee, type_args, args) => {
                self.print_expr_prec(callee, PREC_POSTFIX);
                self.out.push_str("::<");
                let mut first = true;
                for ty in type_args {
                    if !first {
                        self.out.push_str(", ");
                    }
                    self.print_type_expr(ty);
                    first = false;
                }
                self.out.push('>');
                self.out.push('(');
                first = true;
                for arg in args {
                    if !first {
                        self.out.push_str(", ");
                    }
                    self.print_arg(arg);
                    first = false;
                }
                self.out.push(')');
            }
            ExprKind::Index(e, idx) => {
                self.print_expr_prec(e, PREC_POSTFIX);
                self.out.push('[');
                self.print_expr(idx);
                self.out.push(']');
            }
            ExprKind::Propagate(e) => {
                self.print_expr_prec(e, PREC_POSTFIX);
                self.out.push('?');
            }
            ExprKind::Await(e) => {
                self.print_expr_prec(e, PREC_POSTFIX);
                self.out.push_str(".await");
            }
            ExprKind::If(ie) => self.print_if(ie),
            ExprKind::Match(m) => self.print_match(m),
            ExprKind::For(f) => self.print_for(f),
            ExprKind::While(w) => self.print_while(w),
            ExprKind::Lambda(l) => self.print_lambda(l),
            ExprKind::Spawn(e) => {
                self.out.push_str("spawn ");
                self.print_expr(e);
            }
        }
    }

    /// Print `expr`, wrapping it in parentheses if its binding is weaker than `max_prec`.
    fn print_expr_prec(&mut self, expr: &Expr, max_prec: u8) {
        let needs_parens = match &expr.kind {
            ExprKind::BinaryOp(_, op, _) => bin_op_prec(*op) > max_prec,
            _ => false,
        };
        if needs_parens {
            self.out.push('(');
            self.print_expr(expr);
            self.out.push(')');
        } else {
            self.print_expr(expr);
        }
    }

    fn print_arg(&mut self, arg: &Arg) {
        if let Some(label) = &arg.label {
            self.out.push_str(label);
            self.out.push_str(": ");
        }
        self.print_expr(&arg.value);
    }

    // ── Control flow ─────────────────────────────────────────────────────────

    fn print_if(&mut self, ie: &IfExpr) {
        let indent = self.indent_str();
        let if_line = self.line_of(ie.span.start);
        self.out.push_str("if ");
        self.print_expr(&ie.condition);
        self.out.push('\n');
        self.last_line = self.last_line.max(if_line);
        self.indent += 1;
        self.print_stmts(&ie.then_body);
        self.indent -= 1;
        match &ie.else_body {
            None => {
                self.out.push_str(&indent);
                self.out.push_str("end");
            }
            Some(ElseBranch::Else(stmts)) => {
                self.out.push_str(&indent);
                self.out.push_str("else\n");
                self.indent += 1;
                self.print_stmts(stmts);
                self.indent -= 1;
                let ind2 = self.indent_str();
                self.out.push_str(&ind2);
                self.out.push_str("end");
            }
            Some(ElseBranch::ElseIf(nested)) => {
                self.out.push_str(&indent);
                self.out.push_str("else ");
                self.print_if(nested);
            }
        }
    }

    fn print_match(&mut self, m: &MatchExpr) {
        let indent = self.indent_str();
        let match_line = self.line_of(m.span.start);
        self.out.push_str("match ");
        self.print_expr(&m.subject);
        self.last_line = self.last_line.max(match_line);
        for arm in &m.arms {
            self.out.push('\n');
            self.out.push_str(&indent);
            self.out.push_str("when ");
            self.print_pattern(&arm.pattern);
            self.out.push('\n');
            self.indent += 1;
            self.print_stmts(&arm.body);
            self.indent -= 1;
        }
        self.out.push_str(&indent);
        self.out.push_str("end");
    }

    fn print_for(&mut self, f: &ForExpr) {
        let indent = self.indent_str();
        let for_line = self.line_of(f.span.start);
        self.out.push_str("for ");
        self.out.push_str(&f.bindings.join(", "));
        self.out.push_str(" in ");
        self.print_expr(&f.iter);
        self.out.push('\n');
        self.last_line = self.last_line.max(for_line);
        self.indent += 1;
        self.print_stmts(&f.body);
        self.indent -= 1;
        self.out.push_str(&indent);
        self.out.push_str("end");
    }

    fn print_while(&mut self, w: &WhileExpr) {
        let indent = self.indent_str();
        let while_line = self.line_of(w.span.start);
        self.out.push_str("while ");
        self.print_expr(&w.condition);
        self.out.push('\n');
        self.last_line = self.last_line.max(while_line);
        self.indent += 1;
        self.print_stmts(&w.body);
        self.indent -= 1;
        self.out.push_str(&indent);
        self.out.push_str("end");
    }

    fn print_lambda(&mut self, l: &LambdaExpr) {
        let indent = self.indent_str();
        let lam_line = self.line_of(l.span.start);
        self.out.push_str("fn(");
        let mut first = true;
        for param in &l.params {
            if !first {
                self.out.push_str(", ");
            }
            self.print_param(param);
            first = false;
        }
        self.out.push(')');
        if let Some(ret) = &l.return_type {
            self.out.push_str(" -> ");
            self.print_type_expr(ret);
        }
        self.out.push('\n');
        self.last_line = self.last_line.max(lam_line);
        self.indent += 1;
        self.print_stmts(&l.body);
        self.indent -= 1;
        self.out.push_str(&indent);
        self.out.push_str("end");
    }

    // ── Patterns ─────────────────────────────────────────────────────────────

    fn print_pattern(&mut self, p: &Pattern) {
        match &p.kind {
            PatternKind::Wildcard => self.out.push('_'),
            PatternKind::Ident(s) => self.out.push_str(s),
            PatternKind::IntLit(n) => self.out.push_str(&n.to_string()),
            PatternKind::FloatLit(f) => {
                let s = if f.is_finite() && f.fract() == 0.0 {
                    format!("{:.1}", f)
                } else {
                    format!("{}", f)
                };
                self.out.push_str(&s);
            }
            PatternKind::StringLit(s) => {
                self.out.push('"');
                self.out.push_str(&escape_string(s));
                self.out.push('"');
            }
            PatternKind::BoolLit(b) => self.out.push_str(if *b { "true" } else { "false" }),
            PatternKind::Constructor(name, fields) => {
                self.out.push_str(name);
                if !fields.is_empty() {
                    self.out.push('(');
                    let mut first = true;
                    for field in fields {
                        if !first {
                            self.out.push_str(", ");
                        }
                        if field.field_name.is_empty() {
                            // Positional / wildcard: `Ok(_)` → field_name is ""
                            self.print_pattern(&field.pattern);
                        } else if matches!(&field.pattern.kind, PatternKind::Ident(n) if n == &field.field_name)
                        {
                            // Shorthand: `Ok(v)` stored as field_name="v", pattern=Ident("v")
                            self.out.push_str(&field.field_name);
                        } else {
                            // Explicit: `Card(last4: binding)`
                            self.out.push_str(&field.field_name);
                            self.out.push_str(": ");
                            self.print_pattern(&field.pattern);
                        }
                        first = false;
                    }
                    self.out.push(')');
                }
            }
        }
    }
}

// ─── Free helpers ─────────────────────────────────────────────────────────────

/// Re-escape a parsed (unescaped) string value back to source form.
fn escape_string(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 4);
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c => out.push(c),
        }
    }
    out
}

/// Precedence levels.  Lower number = tighter binding.
/// PREC_POSTFIX (1) is tighter than any binary op, so receivers of postfix
/// operators never need extra parentheses around binary sub-expressions.
const PREC_POSTFIX: u8 = 1;
const PREC_UNARY: u8 = 2;

fn bin_op_prec(op: BinOp) -> u8 {
    match op {
        BinOp::Mul | BinOp::Div | BinOp::Rem => 3,
        BinOp::Add | BinOp::Sub => 4,
        BinOp::Eq
        | BinOp::NotEq
        | BinOp::Lt
        | BinOp::LtEq
        | BinOp::Gt
        | BinOp::GtEq
        | BinOp::RefEq => 5,
        BinOp::And => 6,
        BinOp::Or => 7,
    }
}

fn bin_op_str(op: BinOp) -> &'static str {
    match op {
        BinOp::Add => "+",
        BinOp::Sub => "-",
        BinOp::Mul => "*",
        BinOp::Div => "/",
        BinOp::Rem => "%",
        BinOp::Eq => "==",
        BinOp::NotEq => "!=",
        BinOp::Lt => "<",
        BinOp::LtEq => "<=",
        BinOp::Gt => ">",
        BinOp::GtEq => ">=",
        BinOp::RefEq => "===",
        BinOp::And => "and",
        BinOp::Or => "or",
    }
}

fn item_span(item: &Item) -> Span {
    match item {
        Item::FnDef(f) => f.span,
        Item::ValueDef(v) => v.span,
        Item::DataDef(d) => d.span,
        Item::TypeDef(t) => t.span,
        Item::TraitDef(t) => t.span,
        Item::ImplDef(i) => i.span,
        Item::Import(i) => i.span,
        Item::Stmt(s) => stmt_span(s),
        Item::TestDef(td) => td.span,
    }
}

fn stmt_span(stmt: &Stmt) -> Span {
    match stmt {
        Stmt::Let(s) => s.span,
        Stmt::Mut(s) => s.span,
        Stmt::Return(s) => s.span,
        Stmt::Defer(s) => s.span,
        Stmt::Break(s) => s.span,
        Stmt::Continue(s) => s.span,
        Stmt::Expr(s) => s.span,
    }
}
