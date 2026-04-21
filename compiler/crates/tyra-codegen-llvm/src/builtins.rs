// Builtin function codegen: print, panic, sys__args, parse__Int.
//
// Extracted from codegen.rs to keep file sizes manageable.

use std::fmt::Write;

use tyra_mir::*;

use crate::codegen::EmitCtx;
use crate::helpers::{is_bool_operand, operand_ref};

/// Try to emit a builtin function call. Returns `true` if handled,
/// `false` if the caller should fall through to user-defined call codegen.
pub(crate) fn emit_builtin_call(
    out: &mut String,
    dest: &Option<String>,
    fname: &str,
    args: &[Operand],
    func: &Function,
    ctx: &EmitCtx,
) -> bool {
    match fname {
        "print" | "eprint" | "println" | "eprintln" => {
            let is_println = fname == "println" || fname == "eprintln";
            emit_print_call(out, dest.as_deref(), args, func, is_println, ctx);
            true
        }
        "panic" => {
            emit_panic_call(out, args, func);
            true
        }
        "sys__args" => {
            emit_sys_args(out, dest.as_deref(), ctx);
            true
        }
        "parse__Int" => {
            emit_parse_int(out, dest.as_deref(), args, func, ctx);
            true
        }
        "__fs_read_raw" => {
            emit_fs_read_raw(out, dest.as_deref(), args, func);
            true
        }
        "__fs_errno" => {
            emit_fs_errno(out, dest.as_deref());
            true
        }
        "__fs_errmsg" => {
            emit_fs_errmsg(out, dest.as_deref());
            true
        }
        "__fs_write_raw" => {
            emit_fs_write_raw(out, args, func);
            true
        }
        "__fs_exists" => {
            emit_fs_exists(out, dest.as_deref(), args, func);
            true
        }
        "__json_parse" => {
            emit_json_call_ptr_to_i64(out, dest.as_deref(), "tyra_json_parse", args, func);
            true
        }
        "__json_err_msg" => {
            emit_json_nullary_ptr(out, dest.as_deref(), "tyra_json_err_msg");
            true
        }
        "__json_err_line" => {
            emit_json_nullary_i64(out, dest.as_deref(), "tyra_json_err_line");
            true
        }
        "__json_err_col" => {
            emit_json_nullary_i64(out, dest.as_deref(), "tyra_json_err_col");
            true
        }
        "__json_kind" => {
            emit_json_i64_to_ptr(out, dest.as_deref(), "tyra_json_kind", args, func);
            true
        }
        "__json_is_string" => {
            emit_json_i64_to_bool(out, dest.as_deref(), "tyra_json_is_string", args, func);
            true
        }
        "__json_is_int" => {
            emit_json_i64_to_bool(out, dest.as_deref(), "tyra_json_is_int", args, func);
            true
        }
        "__json_is_bool" => {
            emit_json_i64_to_bool(out, dest.as_deref(), "tyra_json_is_bool", args, func);
            true
        }
        "__json_str" => {
            emit_json_i64_to_ptr(out, dest.as_deref(), "tyra_json_str", args, func);
            true
        }
        "__json_int" => {
            emit_json_i64_to_i64(out, dest.as_deref(), "tyra_json_int", args, func);
            true
        }
        "__json_bool" => {
            emit_json_i64_to_bool(out, dest.as_deref(), "tyra_json_bool", args, func);
            true
        }
        "__json_get" => {
            emit_json_get(out, dest.as_deref(), args, func);
            true
        }
        "__json_at" => {
            emit_json_at(out, dest.as_deref(), args, func);
            true
        }
        "__http_get" => {
            emit_http_get(out, dest.as_deref(), args, func);
            true
        }
        "__http_status" => {
            emit_http_i64_to_i64(out, dest.as_deref(), "tyra_http_status", args, func);
            true
        }
        "__http_body" => {
            emit_http_body(out, dest.as_deref(), args, func);
            true
        }
        "__http_errno" => {
            emit_http_errno(out, dest.as_deref());
            true
        }
        "__http_errmsg" => {
            emit_http_errmsg(out, dest.as_deref());
            true
        }
        "__http_server_new" => {
            emit_http_server_new(out, dest.as_deref());
            true
        }
        "__http_server_route" => {
            emit_http_server_route(out, dest.as_deref(), args, func);
            true
        }
        "__http_server_listen" => {
            emit_http_server_listen(out, dest.as_deref(), args, func);
            true
        }
        "sys__exit" => {
            // §17.1: core.sys.exit(_ code: Int) -> Never
            if let Some(arg) = args.first() {
                let val = operand_ref(arg, func);
                writeln!(out, "  %{}.i32 = trunc i64 {val} to i32", dest.as_deref().unwrap_or("_exit")).unwrap();
                writeln!(out, "  call void @exit(i32 %{}.i32)", dest.as_deref().unwrap_or("_exit")).unwrap();
            } else {
                writeln!(out, "  call void @exit(i32 0)").unwrap();
            }
            writeln!(out, "  unreachable").unwrap();
            true
        }
        _ => false,
    }
}

/// Emit a print/println call, auto-detecting argument type.
/// String args use %s format, Int args use %ld format, Float args use %g format.
fn emit_print_call(
    out: &mut String,
    dest: Option<&str>,
    args: &[Operand],
    func: &Function,
    is_println: bool,
    ctx: &EmitCtx,
) {
    if args.is_empty() {
        if is_println {
            let call = "call i32 @puts(ptr @.fmt.str)";
            if let Some(d) = dest {
                writeln!(out, "  %{d} = {call}").unwrap();
            } else {
                writeln!(out, "  {call}").unwrap();
            }
        }
        return;
    }

    let arg = &args[0];
    let val = operand_ref(arg, func);

    // Detect type using pre-scanned temp sets
    let is_string = match arg {
        Operand::Const(Constant::StringRef(_)) => true,
        Operand::Var(name) => ctx.string_temps.contains(name),
        _ => false,
    };
    let is_float = match arg {
        Operand::Const(Constant::Float(_)) => true,
        Operand::Var(name) => ctx.float_temps.contains(name),
        _ => false,
    };
    let is_bool = is_bool_operand(arg, ctx);

    let call_str = if is_string && is_println {
        format!("call i32 @puts(ptr {val})")
    } else if is_string {
        format!("call i32 (ptr, ...) @printf(ptr @.fmt.str, ptr {val})")
    } else if is_float {
        let fmt = if is_println {
            "@.fmt.float_ln"
        } else {
            "@.fmt.float"
        };
        format!("call i32 (ptr, ...) @printf(ptr {fmt}, double {val})")
    } else if is_bool {
        // Bool (i1) must be widened to i64 for printf varargs
        let fmt = if is_println {
            "@.fmt.int_ln"
        } else {
            "@.fmt.int"
        };
        // Emit zext inline before the call
        let wide = format!("{val}.wide");
        writeln!(out, "  {wide} = zext i1 {val} to i64").unwrap();
        format!("call i32 (ptr, ...) @printf(ptr {fmt}, i64 {wide})")
    } else {
        let fmt = if is_println {
            "@.fmt.int_ln"
        } else {
            "@.fmt.int"
        };
        format!("call i32 (ptr, ...) @printf(ptr {fmt}, i64 {val})")
    };

    if let Some(d) = dest {
        // printf/puts return i32; widen to i64 so the dest can be used as i64 elsewhere
        writeln!(out, "  %{d}.i32 = {call_str}").unwrap();
        writeln!(out, "  %{d} = sext i32 %{d}.i32 to i64").unwrap();
    } else {
        writeln!(out, "  {call_str}").unwrap();
    }
}

/// §12.1: panic(_ message: String) -> Never
/// Print message to stdout then abort.
fn emit_panic_call(out: &mut String, args: &[Operand], func: &Function) {
    if let Some(arg) = args.first() {
        let val = operand_ref(arg, func);
        writeln!(out, "  call i32 @puts(ptr {val})").unwrap();
    }
    writeln!(out, "  call void @abort()").unwrap();
    writeln!(out, "  unreachable").unwrap();
}

/// §17.1: core.sys.args() -> List<String>
/// Build List<String> from saved argc/argv globals.
fn emit_sys_args(out: &mut String, dest: Option<&str>, ctx: &EmitCtx) {
    let d = dest.unwrap_or("_sys_args");
    let list_ty = if let Some(info) = ctx.struct_map.get("List__String") {
        &info.llvm_name
    } else {
        "%struct.List__String"
    };
    // Load argc and argv (argc is always >= 1: argv[0] is program name)
    writeln!(out, "  %{d}.argc = load i32, ptr @.tyra.argc").unwrap();
    writeln!(out, "  %{d}.argc64 = sext i32 %{d}.argc to i64").unwrap();
    writeln!(out, "  %{d}.argv = load ptr, ptr @.tyra.argv").unwrap();
    // Malloc data array (argc * 8 bytes for ptr array)
    writeln!(out, "  %{d}.size = mul i64 %{d}.argc64, 8").unwrap();
    writeln!(out, "  %{d}.data = call ptr @GC_malloc(i64 %{d}.size)").unwrap();
    // Copy argv pointers into list data using alloca-based loop
    // (alloca avoids phi predecessor issues in non-entry blocks)
    writeln!(out, "  %{d}.ctr = alloca i64").unwrap();
    writeln!(out, "  store i64 0, ptr %{d}.ctr").unwrap();
    writeln!(out, "  br label %{d}.loop").unwrap();
    writeln!(out, "{d}.loop:").unwrap();
    writeln!(out, "  %{d}.i = load i64, ptr %{d}.ctr").unwrap();
    writeln!(out, "  %{d}.done = icmp sge i64 %{d}.i, %{d}.argc64").unwrap();
    writeln!(out, "  br i1 %{d}.done, label %{d}.end, label %{d}.body").unwrap();
    writeln!(out, "{d}.body:").unwrap();
    writeln!(out, "  %{d}.argp = getelementptr ptr, ptr %{d}.argv, i64 %{d}.i").unwrap();
    writeln!(out, "  %{d}.arg = load ptr, ptr %{d}.argp").unwrap();
    writeln!(out, "  %{d}.dstp = getelementptr ptr, ptr %{d}.data, i64 %{d}.i").unwrap();
    writeln!(out, "  store ptr %{d}.arg, ptr %{d}.dstp").unwrap();
    writeln!(out, "  %{d}.next = add i64 %{d}.i, 1").unwrap();
    writeln!(out, "  store i64 %{d}.next, ptr %{d}.ctr").unwrap();
    writeln!(out, "  br label %{d}.loop").unwrap();
    writeln!(out, "{d}.end:").unwrap();
    // Build List struct {ptr, i64}
    writeln!(out, "  %{d}.s0 = insertvalue {list_ty} undef, ptr %{d}.data, 0").unwrap();
    writeln!(out, "  %{d} = insertvalue {list_ty} %{d}.s0, i64 %{d}.argc64, 1").unwrap();
}

/// M10 phase 1: `__fs_read_raw(path: String) -> String`.
/// Delegates to `@tyra_fs_read` in the runtime. Returns empty C string on
/// error; the caller is expected to check `__fs_errno()` to discriminate.
fn emit_fs_read_raw(
    out: &mut String,
    dest: Option<&str>,
    args: &[Operand],
    func: &Function,
) {
    let d = dest.unwrap_or("_fs_read");
    let path = if let Some(arg) = args.first() {
        operand_ref(arg, func)
    } else {
        "null".to_string()
    };
    writeln!(out, "  %{d} = call ptr @tyra_fs_read(ptr {path})").unwrap();
}

/// M10 phase 1: `__fs_errno() -> Int`.
/// Delegates to `@tyra_fs_errno`; widens i32 return to i64.
fn emit_fs_errno(out: &mut String, dest: Option<&str>) {
    let d = dest.unwrap_or("_fs_errno");
    writeln!(out, "  %{d}.i32 = call i32 @tyra_fs_errno()").unwrap();
    writeln!(out, "  %{d} = sext i32 %{d}.i32 to i64").unwrap();
}

/// Follow-up: `__fs_errmsg() -> String`.
/// Delegates to `@tyra_fs_errmsg`; returns a caller-owned C string
/// describing the last IoError (empty for other errno codes).
fn emit_fs_errmsg(out: &mut String, dest: Option<&str>) {
    let d = dest.unwrap_or("_fs_errmsg");
    writeln!(out, "  %{d} = call ptr @tyra_fs_errmsg()").unwrap();
}

/// M10 phase 1b: `__fs_write_raw(path: String, contents: String) -> Unit`.
/// Delegates to `@tyra_fs_write`; caller reads `__fs_errno()` afterward to
/// discriminate success vs failure.
fn emit_fs_write_raw(out: &mut String, args: &[Operand], func: &Function) {
    let path = args
        .first()
        .map(|a| operand_ref(a, func))
        .unwrap_or_else(|| "null".into());
    let contents = args
        .get(1)
        .map(|a| operand_ref(a, func))
        .unwrap_or_else(|| "null".into());
    writeln!(out, "  call void @tyra_fs_write(ptr {path}, ptr {contents})").unwrap();
}

/// M10 phase 1b: `__fs_exists(path: String) -> Bool`.
/// Delegates to `@tyra_fs_exists`; truncates the i32 return to i1.
fn emit_fs_exists(
    out: &mut String,
    dest: Option<&str>,
    args: &[Operand],
    func: &Function,
) {
    let d = dest.unwrap_or("_fs_exists");
    let path = args
        .first()
        .map(|a| operand_ref(a, func))
        .unwrap_or_else(|| "null".into());
    writeln!(out, "  %{d}.i32 = call i32 @tyra_fs_exists(ptr {path})").unwrap();
    writeln!(out, "  %{d} = icmp ne i32 %{d}.i32, 0").unwrap();
}

// ---------------------------------------------------------------------------
// M10 phase 2: JSON intrinsic emit helpers.
// Thin wrappers over the `@tyra_json_*` runtime calls. Handles are i64
// values; 0 is reserved for "error / absent". The stdlib/json.tyra wrapper
// builds Option/Result on top of these primitives.
// ---------------------------------------------------------------------------

fn emit_json_call_ptr_to_i64(
    out: &mut String,
    dest: Option<&str>,
    callee: &str,
    args: &[Operand],
    func: &Function,
) {
    let d = dest.unwrap_or("_json");
    let arg = args
        .first()
        .map(|a| operand_ref(a, func))
        .unwrap_or_else(|| "null".into());
    writeln!(out, "  %{d} = call i64 @{callee}(ptr {arg})").unwrap();
}

fn emit_json_nullary_ptr(out: &mut String, dest: Option<&str>, callee: &str) {
    let d = dest.unwrap_or("_json");
    writeln!(out, "  %{d} = call ptr @{callee}()").unwrap();
}

fn emit_json_nullary_i64(out: &mut String, dest: Option<&str>, callee: &str) {
    let d = dest.unwrap_or("_json");
    writeln!(out, "  %{d} = call i64 @{callee}()").unwrap();
}

fn emit_json_i64_to_ptr(
    out: &mut String,
    dest: Option<&str>,
    callee: &str,
    args: &[Operand],
    func: &Function,
) {
    let d = dest.unwrap_or("_json");
    let arg = args
        .first()
        .map(|a| operand_ref(a, func))
        .unwrap_or_else(|| "0".into());
    writeln!(out, "  %{d} = call ptr @{callee}(i64 {arg})").unwrap();
}

fn emit_json_i64_to_bool(
    out: &mut String,
    dest: Option<&str>,
    callee: &str,
    args: &[Operand],
    func: &Function,
) {
    let d = dest.unwrap_or("_json");
    let arg = args
        .first()
        .map(|a| operand_ref(a, func))
        .unwrap_or_else(|| "0".into());
    writeln!(out, "  %{d}.i32 = call i32 @{callee}(i64 {arg})").unwrap();
    writeln!(out, "  %{d} = icmp ne i32 %{d}.i32, 0").unwrap();
}

fn emit_json_i64_to_i64(
    out: &mut String,
    dest: Option<&str>,
    callee: &str,
    args: &[Operand],
    func: &Function,
) {
    let d = dest.unwrap_or("_json");
    let arg = args
        .first()
        .map(|a| operand_ref(a, func))
        .unwrap_or_else(|| "0".into());
    writeln!(out, "  %{d} = call i64 @{callee}(i64 {arg})").unwrap();
}

fn emit_json_get(out: &mut String, dest: Option<&str>, args: &[Operand], func: &Function) {
    let d = dest.unwrap_or("_json_get");
    let handle = args
        .first()
        .map(|a| operand_ref(a, func))
        .unwrap_or_else(|| "0".into());
    let key = args
        .get(1)
        .map(|a| operand_ref(a, func))
        .unwrap_or_else(|| "null".into());
    writeln!(out, "  %{d} = call i64 @tyra_json_get(i64 {handle}, ptr {key})").unwrap();
}

fn emit_json_at(out: &mut String, dest: Option<&str>, args: &[Operand], func: &Function) {
    let d = dest.unwrap_or("_json_at");
    let handle = args
        .first()
        .map(|a| operand_ref(a, func))
        .unwrap_or_else(|| "0".into());
    let index = args
        .get(1)
        .map(|a| operand_ref(a, func))
        .unwrap_or_else(|| "0".into());
    writeln!(out, "  %{d} = call i64 @tyra_json_at(i64 {handle}, i64 {index})").unwrap();
}

// ---------------------------------------------------------------------------
// M11 phase 1: http client emit helpers.
// ---------------------------------------------------------------------------

fn emit_http_get(out: &mut String, dest: Option<&str>, args: &[Operand], func: &Function) {
    let d = dest.unwrap_or("_http_get");
    let url = args
        .first()
        .map(|a| operand_ref(a, func))
        .unwrap_or_else(|| "null".into());
    writeln!(out, "  %{d} = call i64 @tyra_http_get(ptr {url})").unwrap();
}

fn emit_http_i64_to_i64(
    out: &mut String,
    dest: Option<&str>,
    callee: &str,
    args: &[Operand],
    func: &Function,
) {
    let d = dest.unwrap_or("_http");
    let arg = args
        .first()
        .map(|a| operand_ref(a, func))
        .unwrap_or_else(|| "0".into());
    writeln!(out, "  %{d} = call i64 @{callee}(i64 {arg})").unwrap();
}

fn emit_http_body(out: &mut String, dest: Option<&str>, args: &[Operand], func: &Function) {
    let d = dest.unwrap_or("_http_body");
    let arg = args
        .first()
        .map(|a| operand_ref(a, func))
        .unwrap_or_else(|| "0".into());
    writeln!(out, "  %{d} = call ptr @tyra_http_body(i64 {arg})").unwrap();
}

fn emit_http_errno(out: &mut String, dest: Option<&str>) {
    let d = dest.unwrap_or("_http_errno");
    writeln!(out, "  %{d}.i32 = call i32 @tyra_http_errno()").unwrap();
    writeln!(out, "  %{d} = sext i32 %{d}.i32 to i64").unwrap();
}

fn emit_http_errmsg(out: &mut String, dest: Option<&str>) {
    let d = dest.unwrap_or("_http_errmsg");
    writeln!(out, "  %{d} = call ptr @tyra_http_errmsg()").unwrap();
}

// ---------------------------------------------------------------------------
// M11 phase 2: http server emit helpers.
// ---------------------------------------------------------------------------

fn emit_http_server_new(out: &mut String, dest: Option<&str>) {
    let d = dest.unwrap_or("_srv_new");
    // Runtime returns ptr; Tyra stores the handle as Int. ptrtoint here
    // so downstream MIR sees an i64 consistently with AppServer._handle.
    //
    // TODO(v0.2): the ptrtoint/inttoptr round-trip through AppServer._handle
    // strips LLVM pointer provenance. Safe today because the handle is
    // never dereferenced in Tyra IR (only passed back to opaque extern
    // calls that LLVM cannot alias-analyze). If provenance-based
    // optimizations ever break this, typing `_handle` as a true opaque
    // `ptr` in Tyra would eliminate the round-trip. Tracked under the
    // "opaque handle types" spec follow-up.
    writeln!(out, "  %{d}.ptr = call ptr @tyra_http_server_new()").unwrap();
    writeln!(out, "  %{d} = ptrtoint ptr %{d}.ptr to i64").unwrap();
}

/// `__http_server_route(srv, method, path, handler)` — srv is Int in
/// the MIR (handle); cast back to ptr for the call. handler is emitted
/// as ptr (Tyra function identifier resolves to an LLVM function symbol);
/// the runtime casts it to the expected `fn(*Request)->*Response` sig.
///
/// Intermediate SSA temp name is keyed by `dest` (the Call's dest temp,
/// allocated by lower_expr's `fresh_temp`). `dest` is always unique
/// across calls in the same function, so this avoids the name-collision
/// that a handler-operand-derived tag would hit when the same handler
/// expression is used in two distinct calls within a single function.
fn emit_http_server_route(
    out: &mut String,
    dest: Option<&str>,
    args: &[Operand],
    func: &Function,
) {
    // The `.sptr` intermediate SSA temp is keyed off `dest`, which must
    // be a per-call `fresh_temp()`. `lower_call` always emits one today.
    // Assert defensively so a future refactor that drops dest for void
    // returns fails loudly here rather than producing invalid LLVM IR
    // (duplicate `%_srv_route.sptr` definitions).
    let d = dest.expect(
        "emit_http_server_route requires a fresh dest temp for its .sptr \
         intermediate; a `dest: None` void call would collide on repeat \
         invocations within the same function",
    );
    let srv = args
        .first()
        .map(|a| operand_ref(a, func))
        .unwrap_or_else(|| "0".into());
    let method = args
        .get(1)
        .map(|a| operand_ref(a, func))
        .unwrap_or_else(|| "null".into());
    let path = args
        .get(2)
        .map(|a| operand_ref(a, func))
        .unwrap_or_else(|| "null".into());
    let handler = args
        .get(3)
        .map(|a| operand_ref(a, func))
        .unwrap_or_else(|| "null".into());
    writeln!(out, "  %{d}.sptr = inttoptr i64 {srv} to ptr").unwrap();
    writeln!(
        out,
        "  call void @tyra_http_server_route(ptr %{d}.sptr, ptr {method}, ptr {path}, ptr {handler})"
    )
    .unwrap();
}

fn emit_http_server_listen(
    out: &mut String,
    dest: Option<&str>,
    args: &[Operand],
    func: &Function,
) {
    let d = dest.unwrap_or("_srv_listen");
    let srv = args
        .first()
        .map(|a| operand_ref(a, func))
        .unwrap_or_else(|| "0".into());
    let port = args
        .get(1)
        .map(|a| operand_ref(a, func))
        .unwrap_or_else(|| "0".into());
    writeln!(out, "  %{d}.sptr = inttoptr i64 {srv} to ptr").unwrap();
    writeln!(
        out,
        "  %{d}.i32 = call i32 @tyra_http_server_listen(ptr %{d}.sptr, i64 {port})"
    )
    .unwrap();
    writeln!(out, "  %{d} = sext i32 %{d}.i32 to i64").unwrap();
}

/// parse::<Int>(str) -> Option<Int>
/// Uses strtol with endptr check.
fn emit_parse_int(
    out: &mut String,
    dest: Option<&str>,
    args: &[Operand],
    func: &Function,
    ctx: &EmitCtx,
) {
    let d = dest.unwrap_or("_parse");
    let opt_ty = if let Some(info) = ctx.struct_map.get("Option__Int") {
        &info.llvm_name
    } else {
        "%struct.Option__Int"
    };
    let val = if let Some(arg) = args.first() {
        operand_ref(arg, func)
    } else {
        "null".to_string()
    };
    // Alloca for endptr
    writeln!(out, "  %{d}.endp = alloca ptr").unwrap();
    // Call strtol(str, &endptr, 10)
    writeln!(out, "  %{d}.val = call i64 @strtol(ptr {val}, ptr %{d}.endp, i32 10)").unwrap();
    // Load endptr
    writeln!(out, "  %{d}.ep = load ptr, ptr %{d}.endp").unwrap();
    // Check: endptr == str means no conversion at all
    writeln!(out, "  %{d}.nconv = icmp eq ptr %{d}.ep, {val}").unwrap();
    // Check: *endptr != '\0' means trailing garbage (partial parse)
    writeln!(out, "  %{d}.epch = load i8, ptr %{d}.ep").unwrap();
    writeln!(out, "  %{d}.partial = icmp ne i8 %{d}.epch, 0").unwrap();
    writeln!(out, "  %{d}.fail = or i1 %{d}.nconv, %{d}.partial").unwrap();
    writeln!(out, "  %{d}.slot = alloca {opt_ty}").unwrap();
    writeln!(out, "  br i1 %{d}.fail, label %{d}.none, label %{d}.some").unwrap();
    // Some path
    writeln!(out, "{d}.some:").unwrap();
    writeln!(out, "  %{d}.some.s0 = insertvalue {opt_ty} undef, i8 0, 0").unwrap();
    writeln!(out, "  %{d}.some.v = insertvalue {opt_ty} %{d}.some.s0, i64 %{d}.val, 1").unwrap();
    writeln!(out, "  store {opt_ty} %{d}.some.v, ptr %{d}.slot").unwrap();
    writeln!(out, "  br label %{d}.merge").unwrap();
    // None path
    writeln!(out, "{d}.none:").unwrap();
    writeln!(out, "  %{d}.none.s0 = insertvalue {opt_ty} undef, i8 1, 0").unwrap();
    writeln!(out, "  %{d}.none.v = insertvalue {opt_ty} %{d}.none.s0, i64 0, 1").unwrap();
    writeln!(out, "  store {opt_ty} %{d}.none.v, ptr %{d}.slot").unwrap();
    writeln!(out, "  br label %{d}.merge").unwrap();
    // Merge
    writeln!(out, "{d}.merge:").unwrap();
    writeln!(out, "  %{d} = load {opt_ty}, ptr %{d}.slot").unwrap();
}
