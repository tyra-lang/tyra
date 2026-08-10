// Affine task-handle tracking (ADR-0030, spec §14.5).
//
// The runtime treats task handles as consumed-exactly-once: `tyra_task_await`
// does `Arc::from_raw`, so awaiting the same handle twice is a double-free
// (undefined behavior). This pass is a conservative, purely syntactic check
// that catches the double-await / never-awaited cases that are statically
// visible without any type information — it deliberately does not attempt to
// track handles that escape the local scope (stored, passed, returned,
// captured). See ADR-0030 for the full lattice and the documented gap.
//
// This is a standalone AST pass, run once per function-like body (fn bodies,
// impl methods, and the implicit top-level `main`) from `checker::check`,
// alongside the existing warning passes. It does not participate in
// `infer_expr` and needs no `TypeEnv` — `spawn`, `.await` on a bare
// identifier, and `tasks.join_all([...])` / `tasks.select([...])` are all
// syntactically identifiable.
//
// Shadowing (ADR-0030 addendum): a scope's `Frame` records every `let` /
// `mut` / tuple-destructure / for-binding / match-pattern-binding / lambda
// param name it sees, not only the ones bound to a tracked handle form. A
// non-tracked binding is recorded as `Binding::Untracked` — a shadow
// tombstone — so that name lookup for a shadowing inner binding stops at
// that scope instead of falling through to an outer tracked handle of the
// same name. Without this, `let t = spawn f(); t.await; let t = 5; t.await`
// (an inner non-tracked `t`) would otherwise mutate the OUTER handle's
// consume state and produce false diagnostics.

use std::collections::HashSet;

use tyra_ast::*;
use tyra_diagnostics::{Diagnostic, Label, Report, Span};

/// State of a single tracked handle's consumption, per the ADR-0030 lattice.
#[derive(Debug, Clone, Copy)]
enum ConsumeState {
    /// Not yet consumed on any known path.
    Live,
    /// Consumed on some but not all incoming paths (branch merge). Consuming
    /// a `MaybeConsumed` handle is allowed and transitions it to `Consumed`.
    MaybeConsumed,
    /// Definitely consumed; the span is the first consume site, kept for the
    /// "first consumed here" label on a subsequent double-await diagnostic.
    Consumed(Span),
}

/// A tracked handle: a `let`-bound local created by `spawn` or
/// `tasks.select([...])`.
#[derive(Debug, Clone)]
struct Handle {
    /// Span of the `spawn` / `tasks.select(...)` expression that created
    /// this handle — used for the "spawned here" / "task spawned here"
    /// labels.
    spawn_span: Span,
    /// `loop_depth` at the point this binding was created. Consuming the
    /// handle from a strictly deeper loop nesting than this means the
    /// consume site can run more than once per handle (spec §14.5).
    loop_depth: u32,
    state: ConsumeState,
}

/// One binding slot in a `Frame`: either a tracked task handle, or a shadow
/// tombstone recording that this name is bound to something else in this
/// scope (so lookups must stop here rather than reach an outer frame).
#[derive(Debug, Clone)]
enum Binding {
    Tracked(Handle),
    /// Bound in this scope to a non-tracked value (plain `let`/`mut`,
    /// tuple-destructure element, for-loop binding, match-pattern binding,
    /// lambda param, or a handle that has escaped / been consumed past
    /// diagnosability). Absorbing: never diagnosed, and shadows any outer
    /// binding of the same name for the remainder of this scope.
    Untracked,
}

/// One lexical scope's bindings, by name (tracked handles and shadow
/// tombstones alike).
type Frame = std::collections::HashMap<String, Binding>;

/// Immutable-per-call-site pass state threaded through every visitor
/// function, alongside the mutable `stack: &mut Vec<Frame>`.
#[derive(Clone, Copy)]
struct Ctx<'a> {
    /// Local identifiers that resolve to the `core.tasks` module (the
    /// literal `tasks` plus any `import core.tasks as <alias>` alias) —
    /// see `collect_tasks_aliases`.
    tasks_aliases: &'a HashSet<String>,
    /// Current loop nesting depth (spec §14.5's loop-repeat-await rule).
    loop_depth: u32,
    /// What kind of unconditional exit (if any) is guaranteed to follow the
    /// current statement in its own statement list — used to suppress the
    /// loop-depth E0321 rule for that statement's consumes when the exit
    /// provably bounds the consume to a single run (finding 5a). `Return`
    /// suppresses at any loop depth (it leaves the function entirely);
    /// `Break` suppresses only when the handle was bound directly outside
    /// the loop being broken out of (`handle.loop_depth + 1 ==
    /// ctx.loop_depth`), since a `break` only unwinds one loop level. A
    /// `continue` reachable before the break/return on the same path
    /// defeats the proof — the loop can iterate again — so it forces
    /// `LoopExit::None` (see `loop_exit_follows`/`contains_continue`).
    /// Recomputed fresh per statement list by `visit_stmts`; never
    /// inherited across a nested statement list, and explicitly reset to
    /// `LoopExit::None` when entering a `for`/`while` body or condition
    /// (an exit statement lexically outside the loop cannot retroactively
    /// prove anything about the loop's own body).
    loop_exit: LoopExit,
}

/// What kind of unconditional control-flow exit follows the current
/// statement in its own statement list.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum LoopExit {
    /// None (or provably unreachable because a `continue` intervenes).
    None,
    /// A top-level `break` — exits exactly ONE loop level.
    Break,
    /// A top-level `return` — exits the function (all loop levels).
    Return,
}

/// Entry point: run the affine pass over every function-like body in the
/// file — free functions, impl methods, and the implicit top-level `main`
/// (ADR-0006 Rule 5) formed by every top-level `Item::Stmt` in source order.
/// `Item::TestDef` is desugared to `Item::FnDef` by the driver before this
/// runs (see `tyra_ast::TestDef` doc comment), so test bodies are covered by
/// the `Item::FnDef` arm below without special-casing.
pub fn check_task_affine(items: &[Item], report: &mut Report) {
    let tasks_aliases = collect_tasks_aliases(items);
    let base_ctx = Ctx {
        tasks_aliases: &tasks_aliases,
        loop_depth: 0,
        loop_exit: LoopExit::None,
    };

    let top_level_stmts: Vec<&Stmt> = items
        .iter()
        .filter_map(|item| match item {
            Item::Stmt(s) => Some(s),
            _ => None,
        })
        .collect();

    let mut top_level: Vec<Frame> = vec![Frame::new()];
    for stmt in top_level_stmts.iter() {
        let stmt_ctx = base_ctx;
        visit_stmt(stmt, &mut top_level, stmt_ctx, report);
        if matches!(stmt, Stmt::Return(_) | Stmt::Break(_) | Stmt::Continue(_)) {
            break;
        }
    }
    if !top_level_stmts.is_empty() {
        pop_scope(&mut top_level, report);
    }

    for item in items {
        match item {
            Item::FnDef(f) => analyze_body(&f.body, &param_names(f), &tasks_aliases, report),
            Item::ImplDef(i) => {
                for m in &i.methods {
                    analyze_body(&m.body, &param_names(m), &tasks_aliases, report);
                }
            }
            _ => {}
        }
    }
}

fn analyze_body(
    body: &[Stmt],
    params: &[String],
    tasks_aliases: &HashSet<String>,
    report: &mut Report,
) {
    let mut stack: Vec<Frame> = vec![Frame::new()];
    for p in params {
        tombstone(&mut stack, p, report);
    }
    let ctx = Ctx {
        tasks_aliases,
        loop_depth: 0,
        loop_exit: LoopExit::None,
    };
    visit_stmts(body, &mut stack, ctx, report);
    pop_scope(&mut stack, report);
}

/// Build the set of local identifiers that resolve to `core.tasks`,
/// mirroring the local→canonical alias mapping `tyra-mir/src/lower/mod.rs`
/// builds for module-qualified call resolution (~lines 110-129): the local
/// name is the import's alias if present, else the last path segment: any
/// import whose *canonical* last path segment is `tasks` (i.e. resolves to
/// `core.tasks`) registers its local name here. This keeps the affine pass
/// type-info-free — it only consults the import list, never MIR/type data.
fn param_names(f: &FnDef) -> Vec<String> {
    let mut names: Vec<String> = f.params.iter().map(|p| p.name.clone()).collect();
    if f.self_param.is_some() {
        names.push("self".to_string());
    }
    names
}

fn collect_tasks_aliases(items: &[Item]) -> HashSet<String> {
    let mut aliases = HashSet::new();
    for item in items {
        if let Item::Import(imp) = item {
            let canonical = imp.path.last().map(String::as_str).unwrap_or_default();
            if canonical == "tasks" {
                let local_name = imp.alias.as_deref().unwrap_or(canonical);
                aliases.insert(local_name.to_string());
            }
        }
    }
    aliases
}

/// Whether the remaining statements in a list (from the current one to the
/// end, inclusive) contain a top-level `break` or `return` (finding 5a's
/// lenient suppression rule — see `Ctx::loop_exit_follows`). Deliberately
/// does not recurse into nested statement lists (if/match/for bodies): only
/// a `break`/`return` in the *same* list, at or after the current statement,
/// counts.
fn loop_exit_follows(remaining: &[Stmt]) -> LoopExit {
    for stmt in remaining {
        match stmt {
            Stmt::Return(s) => {
                // A `continue` nested in the RETURNED VALUE runs before the
                // return itself — the loop can still iterate again.
                if s.value.as_ref().is_some_and(|v| expr_contains_continue(v)) {
                    return LoopExit::None;
                }
                return LoopExit::Return;
            }
            Stmt::Break(_) => return LoopExit::Break,
            // A `continue` reached before the break/return makes that exit
            // unreachable on this path — the loop can iterate again.
            Stmt::Continue(_) => return LoopExit::None,
            other => {
                if contains_continue(other) {
                    return LoopExit::None;
                }
            }
        }
    }
    LoopExit::None
}

/// Whether `stmt` contains a `continue` that targets the CURRENT loop
/// (i.e. not one nested inside another loop). Purely syntactic.
fn contains_continue(stmt: &Stmt) -> bool {
    match stmt {
        Stmt::Continue(_) => true,
        Stmt::Expr(s) => expr_contains_continue(&s.expr),
        Stmt::Let(s) => expr_contains_continue(&s.value),
        Stmt::TupleLet(s) => expr_contains_continue(&s.value),
        Stmt::Mut(s) => expr_contains_continue(&s.value),
        Stmt::Return(s) => s.value.as_ref().is_some_and(|v| expr_contains_continue(v)),
        Stmt::Defer(s) => expr_contains_continue(&s.expr),
        Stmt::Break(_) => false,
    }
}

fn expr_contains_continue(expr: &Expr) -> bool {
    match &expr.kind {
        // Nested loops own their own `continue`s.
        ExprKind::For(_) | ExprKind::While(_) | ExprKind::Lambda(_) => false,
        ExprKind::If(if_expr) => if_contains_continue(if_expr),
        ExprKind::Match(m) => m
            .arms
            .iter()
            .any(|a| a.body.iter().any(contains_continue)),
        _ => false,
    }
}

fn if_contains_continue(if_expr: &IfExpr) -> bool {
    if if_expr.then_body.iter().any(contains_continue) {
        return true;
    }
    match &if_expr.else_body {
        Some(ElseBranch::Else(body)) => body.iter().any(contains_continue),
        Some(ElseBranch::ElseIf(inner)) => if_contains_continue(inner),
        None => false,
    }
}

// ---------------------------------------------------------------------------
// Scope stack primitives
// ---------------------------------------------------------------------------

fn push_scope(stack: &mut Vec<Frame>) {
    stack.push(Frame::new());
}

fn warn_never_awaited(spawn_span: Span, report: &mut Report) {
    report.add(
        Diagnostic::warning("task handle is never awaited")
            .with_code("E0322")
            .with_label(Label::new(spawn_span, "spawned here"))
            .with_note(
                "the task still runs to completion but its result is discarded \
                 and the handle leaks",
            )
            .with_help("await it or pass it to tasks.join_all"),
    );
}

/// Pop the innermost scope, warning (E0322) on every handle still `Live`.
/// `MaybeConsumed`, `Consumed`, and `Untracked` are silent — see the
/// branch-merge lattice.
fn pop_scope(stack: &mut Vec<Frame>, report: &mut Report) {
    let Some(frame) = stack.pop() else { return };
    for binding in frame.into_values() {
        if let Binding::Tracked(handle) = binding {
            if matches!(handle.state, ConsumeState::Live) {
                warn_never_awaited(handle.spawn_span, report);
            }
        }
    }
}

/// Register a new tracked handle in the innermost scope. If this name
/// already holds a `Live` tracked handle in the SAME frame, that handle is
/// about to be silently lost (overwritten before ever being awaited) — emit
/// its E0322 now, since it will never again be visible to `pop_scope`.
/// Deeper-scope shadows are unaffected: they are handled by their own
/// scope's `pop_scope`, not here.
fn define(stack: &mut [Frame], loop_depth: u32, name: String, spawn_span: Span, report: &mut Report) {
    let Some(frame) = stack.last_mut() else {
        return;
    };
    if let Some(Binding::Tracked(prev)) = frame.get(&name) {
        if matches!(prev.state, ConsumeState::Live) {
            warn_never_awaited(prev.spawn_span, report);
        }
    }
    frame.insert(
        name,
        Binding::Tracked(Handle {
            spawn_span,
            loop_depth,
            state: ConsumeState::Live,
        }),
    );
}

/// Record a shadow tombstone for a non-tracked binding in the innermost
/// scope (plain `let`/`mut`, tuple-destructure element, for-loop binding,
/// match-pattern binding, lambda param). Ensures a lookup for this name
/// inside the current scope stops here instead of reaching an outer tracked
/// handle of the same name.
fn tombstone(stack: &mut [Frame], name: &str, report: &mut Report) {
    if let Some(frame) = stack.last_mut() {
        if let Some(Binding::Tracked(prev)) = frame.get(name) {
            if matches!(prev.state, ConsumeState::Live) {
                let span = prev.spawn_span;
                warn_never_awaited(span, report);
            }
        }
        frame.insert(name.to_string(), Binding::Untracked);
    }
}

/// Escape a tracked handle: mark it `Untracked` so it is never diagnosed
/// again (ADR-0030's "absorbing Untracked" state), without removing the key
/// — the key must stay present so it continues to shadow any outer binding
/// of the same name. No-op if `name` is not bound in any frame.
fn escape(stack: &mut [Frame], name: &str) {
    if let Some(frame) = stack.iter_mut().rev().find(|f| f.contains_key(name)) {
        frame.insert(name.to_string(), Binding::Untracked);
    }
}

/// Consume a tracked handle at `span` (`.await` on it, or as a direct
/// `tasks.join_all([...])` list element). No-op if `name` is not bound
/// anywhere, or bound only as a shadow tombstone (`Untracked`).
fn consume(stack: &mut [Frame], ctx: Ctx, name: &str, span: Span, report: &mut Report) {
    let Some(frame) = stack.iter_mut().rev().find(|f| f.contains_key(name)) else {
        return;
    };
    let binding = frame.get_mut(name).expect("just checked contains_key");
    let Binding::Tracked(handle) = binding else {
        return; // shadow tombstone / escaped — untracked, no diagnostics
    };

    // A consume site strictly inside a loop deeper than where the handle was
    // bound can run more than once per handle at runtime — flag it
    // unconditionally, independent of the Live/MaybeConsumed/Consumed state
    // (a single static occurrence here is still a real repeat-await risk).
    // Suppressed when a top-level break/return in the same statement list
    // proves this consume runs at most once per handle (finding 5a).
    let exit_proves_single_run = match ctx.loop_exit {
        // `return` leaves the function: the consume runs at most once.
        LoopExit::Return => true,
        // `break` exits exactly ONE loop level: it only proves a single run
        // when the handle was bound directly outside that loop.
        LoopExit::Break => handle.loop_depth + 1 == ctx.loop_depth,
        LoopExit::None => false,
    };
    if handle.loop_depth < ctx.loop_depth && !exit_proves_single_run {
        report.add(
            Diagnostic::error("task handle may be awaited more than once (awaited inside a loop)")
                .with_code("E0321")
                .with_label(Label::new(span, "awaited inside a loop here"))
                .with_label(Label::new(
                    handle.spawn_span,
                    "task spawned here, outside the loop",
                ))
                .with_note(
                    "double await of a task handle is undefined behavior in the runtime (ADR-0030)",
                )
                .with_help(
                    "spawn a fresh handle inside the loop body, or await this handle \
                 exactly once outside the loop",
                ),
        );
        handle.state = ConsumeState::Consumed(span);
        return;
    }

    match handle.state {
        ConsumeState::Consumed(first_consume_span) => {
            report.add(
                Diagnostic::error("task handle is awaited more than once")
                    .with_code("E0321")
                    .with_label(Label::new(span, "awaited again here"))
                    .with_label(Label::new(first_consume_span, "first consumed here"))
                    .with_note(
                        "double await of a task handle is undefined behavior in the runtime (ADR-0030)",
                    )
                    .with_help("bind the result of the first await and reuse it"),
            );
        }
        ConsumeState::Live | ConsumeState::MaybeConsumed => {
            handle.state = ConsumeState::Consumed(span);
        }
    }
}

/// Check a `tasks.select([...])` source element without consuming it
/// (mirrors `tyra_task_select`'s Arc-clone semantics — the handle stays
/// exactly as it was and must still be awaited individually). The one
/// diagnosable misuse here is passing an already-`Consumed` handle: the
/// runtime does `Arc::increment_strong_count` on every source at the select
/// call site, which is UB on a handle whose Arc has already been reclaimed
/// by a prior `.await` (finding 1). The loop-depth rule intentionally does
/// NOT apply here — repeated `select` of a `Live` handle across loop
/// iterations is safe (select never consumes).
fn check_select_source(stack: &mut [Frame], name: &str, span: Span, report: &mut Report) {
    let Some(frame) = stack.iter_mut().rev().find(|f| f.contains_key(name)) else {
        return;
    };
    let binding = frame.get_mut(name).expect("just checked contains_key");
    let Binding::Tracked(handle) = binding else {
        return; // shadow tombstone / escaped — untracked, no diagnostics
    };
    if let ConsumeState::Consumed(first_span) = handle.state {
        report.add(
            Diagnostic::error("task handle passed to tasks.select after it was consumed")
                .with_code("E0321")
                .with_label(Label::new(span, "passed to tasks.select here"))
                .with_label(Label::new(first_span, "first consumed here"))
                .with_note(
                    "the runtime increments the handle's reference count for every select \
                     source at the call site — passing an already-consumed handle here is \
                     undefined behavior (ADR-0030)",
                )
                .with_help("only pass handles to tasks.select before they are individually awaited"),
        );
    }
    // Live / MaybeConsumed: select does not consume — state is left exactly
    // as-is.
}

// ---------------------------------------------------------------------------
// Branch-merge join (if/else, match arms, and/or short-circuit RHS)
// ---------------------------------------------------------------------------

/// Run `body` as a fresh nested scope starting from `snapshot`, tombstoning
/// `extra_tombstones` (e.g. a match arm's pattern bindings) into that new
/// scope before walking `body`. Returns the resulting outer-visible stack
/// (same shape as `snapshot`; the pushed local scope is popped — with its
/// own E0322 checks — before returning).
fn run_branch(
    snapshot: &[Frame],
    extra_tombstones: &[String],
    body: &[Stmt],
    ctx: Ctx,
    report: &mut Report,
) -> Vec<Frame> {
    let mut stack = snapshot.to_vec();
    push_scope(&mut stack);
    for name in extra_tombstones {
        tombstone(&mut stack, name, report);
    }
    visit_stmts(body, &mut stack, ctx, report);
    pop_scope(&mut stack, report);
    stack
}

/// Run a single expression (the RHS of a short-circuit `and`/`or`) as a
/// branch: it may or may not execute at runtime, exactly like an `if` arm.
fn run_expr_branch(snapshot: &[Frame], expr: &Expr, ctx: Ctx, report: &mut Report) -> Vec<Frame> {
    let mut stack = snapshot.to_vec();
    push_scope(&mut stack);
    visit_expr(expr, &mut stack, ctx, report);
    pop_scope(&mut stack, report);
    stack
}

/// Pointwise join across the union of names present in `a` and `b`. A name
/// present as `Untracked` (or missing entirely — defensive; every binding is
/// recorded in-place now, so this should not happen) on either side makes
/// the joined result `Untracked` too (absorbing).
fn join_stacks(a: Vec<Frame>, b: Vec<Frame>) -> Vec<Frame> {
    a.into_iter()
        .zip(b)
        .map(|(fa, fb)| {
            let mut names: HashSet<&String> = fa.keys().collect();
            names.extend(fb.keys());
            let mut out = Frame::new();
            for name in names {
                let ba = fa.get(name);
                let bb = fb.get(name);
                let joined = match (ba, bb) {
                    (Some(Binding::Tracked(ha)), Some(Binding::Tracked(hb))) => {
                        Binding::Tracked(join_handle(ha.clone(), hb.clone()))
                    }
                    _ => Binding::Untracked,
                };
                out.insert(name.clone(), joined);
            }
            out
        })
        .collect()
}

/// Consumed⊔Consumed=Consumed; Live⊔Live=Live; any other combination (one
/// side consumed, the other not, or either side already MaybeConsumed) joins
/// to MaybeConsumed — spec §14.5's documented gap: this resolves toward no
/// false positives rather than catching every real double-await.
fn join_handle(a: Handle, b: Handle) -> Handle {
    let state = match (a.state, b.state) {
        (ConsumeState::Consumed(span), ConsumeState::Consumed(_)) => ConsumeState::Consumed(span),
        (ConsumeState::Live, ConsumeState::Live) => ConsumeState::Live,
        _ => ConsumeState::MaybeConsumed,
    };
    Handle {
        spawn_span: a.spawn_span,
        loop_depth: a.loop_depth,
        state,
    }
}

// ---------------------------------------------------------------------------
// Statement / expression traversal
// ---------------------------------------------------------------------------

fn visit_stmts(stmts: &[Stmt], stack: &mut Vec<Frame>, ctx: Ctx, report: &mut Report) {
    for (i, stmt) in stmts.iter().enumerate() {
        let stmt_ctx = Ctx {
            loop_exit: loop_exit_follows(&stmts[i..]),
            ..ctx
        };
        visit_stmt(stmt, stack, stmt_ctx, report);
        // Everything after an unconditional exit in this list is unreachable
        // — visiting it would report double-await / never-awaited on dead
        // code (finding 2).
        if matches!(stmt, Stmt::Return(_) | Stmt::Break(_) | Stmt::Continue(_)) {
            break;
        }
    }
}

fn visit_stmt(stmt: &Stmt, stack: &mut Vec<Frame>, ctx: Ctx, report: &mut Report) {
    match stmt {
        Stmt::Let(s) => {
            visit_expr(&s.value, stack, ctx, report);
            if let Some(spawn_span) = tracked_rhs_span(&s.value, stack, ctx.tasks_aliases) {
                define(stack, ctx.loop_depth, s.name.clone(), spawn_span, report);
            } else {
                // Non-tracked `let` — shadow tombstone (finding 4).
                tombstone(stack, &s.name, report);
            }
        }
        // Tuple destructure is never a tracked binding form (v1) — walk the
        // RHS generically so nested spawns/awaits/escapes are still seen,
        // and tombstone every destructured name (finding 4).
        Stmt::TupleLet(s) => {
            visit_expr(&s.value, stack, ctx, report);
            for name in &s.bindings {
                tombstone(stack, name, report);
            }
        }
        Stmt::Mut(s) => {
            visit_expr(&s.value, stack, ctx, report);
            tombstone(stack, &s.name, report);
        }
        Stmt::Return(s) => {
            if let Some(v) = &s.value {
                visit_expr(v, stack, ctx, report);
            }
        }
        Stmt::Defer(s) => visit_expr(&s.expr, stack, ctx, report),
        Stmt::Break(_) | Stmt::Continue(_) => {}
        Stmt::Expr(s) => visit_expr(&s.expr, stack, ctx, report),
    }
}

/// If `value` is one of the two tracked-binding RHS shapes (`spawn ...` or
/// `tasks.select([...])`, recognizing any registered `core.tasks` alias),
/// return the span to record as `spawn_span`. `tasks.join_all([...])`'s own
/// result is deliberately excluded — it is not a tracked handle in v1
/// (spec §14.5).
fn tracked_rhs_span(value: &Expr, stack: &[Frame], tasks_aliases: &HashSet<String>) -> Option<Span> {
    match &value.kind {
        ExprKind::Spawn(_) => Some(value.span),
        ExprKind::Call(callee, args) => match classify_tasks_call(callee, args, stack, tasks_aliases) {
            Some(("select", _)) => Some(value.span),
            _ => None,
        },
        _ => None,
    }
}

/// Recognize `<ident>.<method>([...])` where `<ident>` resolves to the
/// `core.tasks` module (the literal `tasks`, or any local alias registered
/// by `collect_tasks_aliases` from `import core.tasks as <alias>` — finding
/// 2) and `<method>` is `join_all` or `select`, with a single list-literal
/// argument. Returns the method name and the list's elements. Still no
/// type/import *resolution* proper — the alias set is built once from the
/// syntactic import list, keeping this pass's "essentially no type info"
/// design.
fn classify_tasks_call<'e>(
    callee: &Expr,
    args: &'e [Arg],
    stack: &[Frame],
    tasks_aliases: &HashSet<String>,
) -> Option<(&'static str, &'e [Expr])> {
    let ExprKind::FieldAccess(obj, method) = &callee.kind else {
        return None;
    };
    let ExprKind::Ident(receiver) = &obj.kind else {
        return None;
    };
    if !tasks_aliases.contains(receiver) {
        return None;
    }
    // A local binding of the same name shadows the module alias (checker.rs
    // skips module dispatch in exactly this case) — bail out rather than
    // treating a user method call as a tasks builtin (finding 3).
    if stack.iter().any(|f| f.contains_key(receiver)) {
        return None;
    }
    let kind = match method.as_str() {
        "join_all" => "join_all",
        "select" => "select",
        _ => return None,
    };
    if args.len() != 1 {
        return None;
    }
    let ExprKind::ListLit(elems) = &args[0].value.kind else {
        return None;
    };
    Some((kind, elems))
}

fn visit_expr(expr: &Expr, stack: &mut Vec<Frame>, ctx: Ctx, report: &mut Report) {
    match &expr.kind {
        // A bare identifier reached in general expression-walking position
        // (function argument, list/tuple/map element, field access receiver,
        // return value, assignment operand, etc.) is exactly the v1 escape
        // rule: anything that is not `.await` on the ident or a blessed
        // list-element position escapes it (ADR-0030).
        ExprKind::Ident(name) => escape(stack, name),

        ExprKind::Await(inner) => {
            if let ExprKind::Ident(name) = &inner.kind {
                consume(stack, ctx, name, expr.span, report);
            } else {
                visit_expr(inner, stack, ctx, report);
            }
        }

        ExprKind::Call(callee, args) => {
            if let Some((kind, elems)) = classify_tasks_call(callee, args, stack, ctx.tasks_aliases) {
                for el in elems {
                    match &el.kind {
                        ExprKind::Ident(name) => match kind {
                            // join_all awaits (consumes) each element.
                            "join_all" => consume(stack, ctx, name, el.span, report),
                            // select does not consume its sources (finding 1
                            // guards the one diagnosable misuse: passing an
                            // already-consumed handle).
                            "select" => check_select_source(stack, name, el.span, report),
                            _ => unreachable!("classify_tasks_call only returns join_all/select"),
                        },
                        _ => visit_expr(el, stack, ctx, report),
                    }
                }
            } else {
                visit_expr(callee, stack, ctx, report);
                for a in args {
                    visit_expr(&a.value, stack, ctx, report);
                }
            }
        }
        ExprKind::TurbofishCall(callee, _, args) => {
            visit_expr(callee, stack, ctx, report);
            for a in args {
                visit_expr(&a.value, stack, ctx, report);
            }
        }

        ExprKind::FieldAccess(inner, _) => visit_expr(inner, stack, ctx, report),

        // `and`/`or` short-circuit (spec §10.1): the RHS may not execute, so
        // it is analyzed as a branch — same snapshot + join used for if-arms
        // — rather than unconditionally (finding 5b). Any other binary
        // operator always evaluates both sides.
        ExprKind::BinaryOp(l, op, r) => match op {
            BinOp::And | BinOp::Or => {
                visit_expr(l, stack, ctx, report);
                let snapshot = stack.clone();
                let rhs_result = run_expr_branch(&snapshot, r, ctx, report);
                *stack = join_stacks(snapshot, rhs_result);
            }
            _ => {
                visit_expr(l, stack, ctx, report);
                visit_expr(r, stack, ctx, report);
            }
        },
        ExprKind::UnaryOp(_, e) => visit_expr(e, stack, ctx, report),
        ExprKind::Assign(l, r) => {
            visit_expr(l, stack, ctx, report);
            visit_expr(r, stack, ctx, report);
        }
        ExprKind::Index(e, i) => {
            visit_expr(e, stack, ctx, report);
            visit_expr(i, stack, ctx, report);
        }
        ExprKind::Propagate(e) => visit_expr(e, stack, ctx, report),

        ExprKind::Tuple(elems) | ExprKind::ListLit(elems) => {
            for e in elems {
                visit_expr(e, stack, ctx, report);
            }
        }
        ExprKind::MapLit(entries) => {
            for (k, v) in entries {
                visit_expr(k, stack, ctx, report);
                visit_expr(v, stack, ctx, report);
            }
        }
        ExprKind::StringInterp(parts) => {
            for p in parts {
                if let StringPart::Expr(e) = p {
                    visit_expr(e, stack, ctx, report);
                }
            }
        }

        ExprKind::If(if_expr) => visit_if(if_expr, stack, ctx, report),
        ExprKind::Match(m) => visit_match(m, stack, ctx, report),
        ExprKind::For(f) => visit_for(f, stack, ctx, report),
        ExprKind::While(w) => visit_while(w, stack, ctx, report),
        ExprKind::Lambda(lam) => visit_lambda(lam, stack, ctx, report),
        ExprKind::Spawn(inner) => visit_expr(inner, stack, ctx, report),

        ExprKind::IntLit(_)
        | ExprKind::FloatLit(_)
        | ExprKind::StringLit(_)
        | ExprKind::BoolLit(_)
        | ExprKind::UnitLit => {}
    }
}

fn visit_if(if_expr: &IfExpr, stack: &mut Vec<Frame>, ctx: Ctx, report: &mut Report) {
    visit_expr(&if_expr.condition, stack, ctx, report);
    let snapshot = stack.clone();
    let then_result = run_branch(&snapshot, &[], &if_expr.then_body, ctx, report);
    let else_result = match &if_expr.else_body {
        Some(ElseBranch::Else(body)) => run_branch(&snapshot, &[], body, ctx, report),
        Some(ElseBranch::ElseIf(inner)) => {
            let mut s = snapshot.clone();
            visit_if(inner, &mut s, ctx, report);
            s
        }
        // No else: the "not taken" path leaves state exactly as it was.
        None => snapshot.clone(),
    };
    *stack = join_stacks(then_result, else_result);
}

fn visit_match(m: &MatchExpr, stack: &mut Vec<Frame>, ctx: Ctx, report: &mut Report) {
    visit_expr(&m.subject, stack, ctx, report);
    let snapshot = stack.clone();
    let mut joined: Option<Vec<Frame>> = None;
    for arm in &m.arms {
        // Pattern bindings (`when Some(n)`) are never tracked (v1), but each
        // bound name gets a shadow tombstone in the arm's own scope so a
        // reference to it inside the arm body cannot alias an outer tracked
        // handle of the same name (finding 4).
        let mut bindings = Vec::new();
        collect_pattern_idents(&arm.pattern, &mut bindings);
        let result = run_branch(&snapshot, &bindings, &arm.body, ctx, report);
        joined = Some(match joined {
            Some(acc) => join_stacks(acc, result),
            None => result,
        });
    }
    if let Some(joined) = joined {
        *stack = joined;
    }
}

/// Collect every identifier bound by a match pattern — `PatternKind::Ident`
/// and constructor-field bindings, recursively (nested constructor/tuple
/// patterns).
fn collect_pattern_idents(pattern: &Pattern, out: &mut Vec<String>) {
    match &pattern.kind {
        PatternKind::Ident(name) => out.push(name.clone()),
        PatternKind::Constructor(_, fields) => {
            for field in fields {
                collect_pattern_idents(&field.pattern, out);
            }
        }
        PatternKind::Tuple(pats) => {
            for p in pats {
                collect_pattern_idents(p, out);
            }
        }
        PatternKind::Wildcard
        | PatternKind::IntLit(_)
        | PatternKind::FloatLit(_)
        | PatternKind::StringLit(_)
        | PatternKind::BoolLit(_) => {}
    }
}

fn visit_for(f: &ForExpr, stack: &mut Vec<Frame>, ctx: Ctx, report: &mut Report) {
    // The iterable evaluates exactly once — stays at the enclosing depth.
    visit_expr(&f.iter, stack, ctx, report);
    push_scope(stack);
    // For-loop pattern bindings are never tracked (v1), but each gets a
    // shadow tombstone in the loop body's scope (finding 4).
    for name in f.bindings.idents() {
        tombstone(stack, name, report);
    }
    let body_ctx = Ctx {
        loop_depth: ctx.loop_depth + 1,
        loop_exit: LoopExit::None,
        ..ctx
    };
    visit_stmts(&f.body, stack, body_ctx, report);
    pop_scope(stack, report);
}

fn visit_while(w: &WhileExpr, stack: &mut Vec<Frame>, ctx: Ctx, report: &mut Report) {
    // Unlike a for-loop's iterable, the condition re-executes every
    // iteration, so a consume inside it can itself run more than once per
    // handle — analyze it at loop_depth + 1, same as the body (finding 3)...
    // unless the body unconditionally exits the loop (top-level break/return,
    // no `continue` reachable first), in which case the condition provably
    // evaluates exactly once and stays at the enclosing depth. This collapse
    // applies to the condition only — never to the body, which would break
    // the `break` suppression rule's depth test in nested loops.
    let body_always_exits = loop_exit_follows(&w.body) != LoopExit::None;
    let body_ctx = Ctx {
        loop_depth: ctx.loop_depth + 1,
        loop_exit: LoopExit::None,
        ..ctx
    };
    let cond_ctx = Ctx {
        loop_depth: if body_always_exits {
            ctx.loop_depth
        } else {
            ctx.loop_depth + 1
        },
        ..body_ctx
    };
    visit_expr(&w.condition, stack, cond_ctx, report);
    push_scope(stack);
    visit_stmts(&w.body, stack, body_ctx, report);
    pop_scope(stack, report);
}

/// A lambda body is analyzed as an independent nested scope (fresh stack,
/// loop_depth reset to 0 — mirrors `TypeEnv::enter_lambda_scope` /
/// `loop_depth = 0` in `checker.rs`). Before that, any outer tracked handle
/// *referenced anywhere* in the lambda body escapes unconditionally — v1
/// does not attempt to reason about whether a captured handle is
/// subsequently awaited correctly inside the closure (spec §14.5). Lambda
/// params get an explicit shadow tombstone in the fresh inner scope
/// (finding 4) — previously safe only incidentally, because the free-ident
/// escape scan below already clears any outer handle a param name could
/// have shadowed; this makes the intent explicit and holds even if that
/// scan's behavior changes.
fn visit_lambda(lam: &LambdaExpr, stack: &mut Vec<Frame>, ctx: Ctx, report: &mut Report) {
    let mut referenced = HashSet::new();
    for stmt in &lam.body {
        collect_idents_stmt(stmt, &mut referenced);
    }
    let outer_tracked: Vec<String> = stack
        .iter()
        .flat_map(|f| f.keys().cloned())
        .filter(|n| referenced.contains(n))
        .collect();
    for name in outer_tracked {
        escape(stack, &name);
    }

    let mut inner: Vec<Frame> = vec![Frame::new()];
    for p in &lam.params {
        tombstone(&mut inner, &p.name, report);
    }
    let inner_ctx = Ctx {
        tasks_aliases: ctx.tasks_aliases,
        loop_depth: 0,
        loop_exit: LoopExit::None,
    };
    visit_stmts(&lam.body, &mut inner, inner_ctx, report);
    pop_scope(&mut inner, report);
}

// ---------------------------------------------------------------------------
// Free-identifier collection (lambda capture detection)
// ---------------------------------------------------------------------------
//
// Deliberately scope-unaware: it collects every identifier name *textually*
// referenced anywhere in a lambda body, including ones that would actually
// resolve to an inner shadowing binding. Over-approximating capture this way
// can only cause an outer handle to escape (silently) when it did not
// strictly need to — never a false-positive diagnostic, consistent with the
// "adjust only toward leniency" design constraint.

fn collect_idents_stmt(stmt: &Stmt, out: &mut HashSet<String>) {
    match stmt {
        Stmt::Let(s) => collect_idents_expr(&s.value, out),
        Stmt::TupleLet(s) => collect_idents_expr(&s.value, out),
        Stmt::Mut(s) => collect_idents_expr(&s.value, out),
        Stmt::Return(s) => {
            if let Some(v) = &s.value {
                collect_idents_expr(v, out);
            }
        }
        Stmt::Defer(s) => collect_idents_expr(&s.expr, out),
        Stmt::Break(_) | Stmt::Continue(_) => {}
        Stmt::Expr(s) => collect_idents_expr(&s.expr, out),
    }
}

fn collect_idents_expr(expr: &Expr, out: &mut HashSet<String>) {
    match &expr.kind {
        ExprKind::Ident(name) => {
            out.insert(name.clone());
        }
        ExprKind::FieldAccess(inner, _) => collect_idents_expr(inner, out),
        ExprKind::BinaryOp(l, _, r) => {
            collect_idents_expr(l, out);
            collect_idents_expr(r, out);
        }
        ExprKind::UnaryOp(_, e) => collect_idents_expr(e, out),
        ExprKind::Assign(l, r) => {
            collect_idents_expr(l, out);
            collect_idents_expr(r, out);
        }
        ExprKind::Call(callee, args) => {
            collect_idents_expr(callee, out);
            for a in args {
                collect_idents_expr(&a.value, out);
            }
        }
        ExprKind::TurbofishCall(callee, _, args) => {
            collect_idents_expr(callee, out);
            for a in args {
                collect_idents_expr(&a.value, out);
            }
        }
        ExprKind::Index(e, i) => {
            collect_idents_expr(e, out);
            collect_idents_expr(i, out);
        }
        ExprKind::Propagate(e) => collect_idents_expr(e, out),
        ExprKind::Await(e) => collect_idents_expr(e, out),
        ExprKind::Tuple(elems) | ExprKind::ListLit(elems) => {
            for e in elems {
                collect_idents_expr(e, out);
            }
        }
        ExprKind::MapLit(entries) => {
            for (k, v) in entries {
                collect_idents_expr(k, out);
                collect_idents_expr(v, out);
            }
        }
        ExprKind::StringInterp(parts) => {
            for p in parts {
                if let StringPart::Expr(e) = p {
                    collect_idents_expr(e, out);
                }
            }
        }
        ExprKind::If(if_expr) => collect_idents_if(if_expr, out),
        ExprKind::Match(m) => {
            collect_idents_expr(&m.subject, out);
            for arm in &m.arms {
                for s in &arm.body {
                    collect_idents_stmt(s, out);
                }
            }
        }
        ExprKind::For(f) => {
            collect_idents_expr(&f.iter, out);
            for s in &f.body {
                collect_idents_stmt(s, out);
            }
        }
        ExprKind::While(w) => {
            collect_idents_expr(&w.condition, out);
            for s in &w.body {
                collect_idents_stmt(s, out);
            }
        }
        ExprKind::Lambda(lam) => {
            for s in &lam.body {
                collect_idents_stmt(s, out);
            }
        }
        ExprKind::Spawn(inner) => collect_idents_expr(inner, out),
        ExprKind::IntLit(_)
        | ExprKind::FloatLit(_)
        | ExprKind::StringLit(_)
        | ExprKind::BoolLit(_)
        | ExprKind::UnitLit => {}
    }
}

fn collect_idents_if(if_expr: &IfExpr, out: &mut HashSet<String>) {
    collect_idents_expr(&if_expr.condition, out);
    for s in &if_expr.then_body {
        collect_idents_stmt(s, out);
    }
    match &if_expr.else_body {
        Some(ElseBranch::Else(body)) => {
            for s in body {
                collect_idents_stmt(s, out);
            }
        }
        Some(ElseBranch::ElseIf(inner)) => collect_idents_if(inner, out),
        None => {}
    }
}
