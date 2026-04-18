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
    writeln!(out, "  %{d}.data = call ptr @malloc(i64 %{d}.size)").unwrap();
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
