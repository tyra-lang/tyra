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
    loc: tyra_mir::SourceLoc,
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
            emit_panic_call(out, args, loc, func, ctx);
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
        "__io_read_line" => {
            let d = dest.as_deref().unwrap_or("_io_read_line");
            writeln!(out, "  %{d} = call ptr @tyra_io_read_line()").unwrap();
            true
        }
        "__io_read_to_end" => {
            let d = dest.as_deref().unwrap_or("_io_read_to_end");
            writeln!(out, "  %{d} = call ptr @tyra_io_read_to_end()").unwrap();
            true
        }
        "__io_eof" => {
            let d = dest.as_deref().unwrap_or("_io_eof");
            writeln!(out, "  %{d}.i32 = call i32 @tyra_io_eof()").unwrap();
            writeln!(out, "  %{d} = trunc i32 %{d}.i32 to i1").unwrap();
            true
        }
        // §17.3.4: string stdlib intrinsics.
        "__string_len" => {
            emit_string_ptr_to_i64(out, dest.as_deref(), "tyra_string_len", args, func);
            true
        }
        "__string_is_empty" => {
            emit_string_ptr_to_bool(out, dest.as_deref(), "tyra_string_is_empty", args, func);
            true
        }
        "__string_trim" => {
            emit_string_ptr_to_ptr(out, dest.as_deref(), "tyra_string_trim", args, func);
            true
        }
        "__string_to_upper" => {
            emit_string_ptr_to_ptr(out, dest.as_deref(), "tyra_string_to_upper", args, func);
            true
        }
        "__string_to_lower" => {
            emit_string_ptr_to_ptr(out, dest.as_deref(), "tyra_string_to_lower", args, func);
            true
        }
        "__string_contains" => {
            emit_string_ptr2_to_bool(out, dest.as_deref(), "tyra_string_contains", args, func);
            true
        }
        "__string_starts_with" => {
            emit_string_ptr2_to_bool(out, dest.as_deref(), "tyra_string_starts_with", args, func);
            true
        }
        "__string_ends_with" => {
            emit_string_ptr2_to_bool(out, dest.as_deref(), "tyra_string_ends_with", args, func);
            true
        }
        "__string_parse_int" => {
            emit_string_ptr_to_i64(out, dest.as_deref(), "tyra_string_parse_int", args, func);
            true
        }
        "__string_parse_errno" => {
            let d = dest.as_deref().unwrap_or("_string_parse_errno");
            writeln!(out, "  %{d}.i32 = call i32 @tyra_string_parse_errno()").unwrap();
            writeln!(out, "  %{d} = sext i32 %{d}.i32 to i64").unwrap();
            true
        }
        "__string_byte_at" => {
            emit_string_ptr_i64_to_i64(out, dest.as_deref(), "tyra_string_byte_at", args, func);
            true
        }
        "__string_substring" => {
            emit_string_ptr_i64_i64_to_ptr(
                out,
                dest.as_deref(),
                "tyra_string_substring",
                args,
                func,
            );
            true
        }
        "__string_reverse" => {
            emit_string_ptr_to_ptr(out, dest.as_deref(), "tyra_string_reverse", args, func);
            true
        }
        "__string_from_byte" => {
            emit_string_i64_to_ptr(out, dest.as_deref(), "tyra_string_from_byte", args, func);
            true
        }
        "__string_split_whitespace" => {
            emit_string_split_ws(out, dest.as_deref(), args, func, ctx);
            true
        }
        "__string_split" => {
            emit_string_split(out, dest.as_deref(), args, func, ctx);
            true
        }
        "__string_replace" => {
            emit_string_replace(out, dest.as_deref(), args, func);
            true
        }
        "__string_join" => {
            emit_string_join(out, dest.as_deref(), args, func, ctx);
            true
        }
        // §17.3.x: time stdlib intrinsics.
        "__time_now_unix" => {
            let d = dest.as_deref().unwrap_or("_time_now_unix");
            writeln!(out, "  %{d} = call i64 @tyra_time_now_unix()").unwrap();
            true
        }
        "__time_monotonic_millis" => {
            let d = dest.as_deref().unwrap_or("_time_monotonic_millis");
            writeln!(out, "  %{d} = call i64 @tyra_time_monotonic_millis()").unwrap();
            true
        }
        // §17.3.x: log stdlib intrinsics.
        "__log_info" => {
            emit_log_call(out, "tyra_log_info", args, func);
            true
        }
        "__log_warn" => {
            emit_log_call(out, "tyra_log_warn", args, func);
            true
        }
        "__log_error" => {
            emit_log_call(out, "tyra_log_error", args, func);
            true
        }
        // §17.3.x: float stdlib intrinsics.
        "__float_eq" => {
            emit_float_double2_to_bool(out, dest.as_deref(), "tyra_float_eq", args, func);
            true
        }
        "__float_approx_eq" => {
            emit_float_approx_eq(out, dest.as_deref(), args, func);
            true
        }
        "__float_abs" => {
            emit_float_double_to_double(out, dest.as_deref(), "tyra_float_abs", args, func);
            true
        }
        "__float_floor" => {
            emit_float_double_to_double(out, dest.as_deref(), "tyra_float_floor", args, func);
            true
        }
        "__float_ceil" => {
            emit_float_double_to_double(out, dest.as_deref(), "tyra_float_ceil", args, func);
            true
        }
        "__float_round" => {
            emit_float_double_to_double(out, dest.as_deref(), "tyra_float_round", args, func);
            true
        }
        "__float_min" => {
            emit_float_double2_to_double(out, dest.as_deref(), "tyra_float_min", args, func);
            true
        }
        "__float_max" => {
            emit_float_double2_to_double(out, dest.as_deref(), "tyra_float_max", args, func);
            true
        }
        "__float_to_string" => {
            emit_float_to_string(out, dest.as_deref(), args, func);
            true
        }
        "__float_parse" => {
            emit_float_parse(out, dest.as_deref(), args, func);
            true
        }
        "__float_parse_errno" => {
            let d = dest.as_deref().unwrap_or("_float_parse_errno");
            writeln!(out, "  %{d}.i32 = call i32 @tyra_float_parse_errno()").unwrap();
            writeln!(out, "  %{d} = sext i32 %{d}.i32 to i64").unwrap();
            true
        }
        "__float_from_int" => {
            emit_float_from_int(out, dest.as_deref(), args, func);
            true
        }
        "__float_to_int" => {
            emit_float_to_int(out, dest.as_deref(), args, func);
            true
        }
        "__float_is_nan" => {
            let d = dest.as_deref().unwrap_or("_float_is_nan");
            let x = args
                .first()
                .map(|a| operand_ref(a, func))
                .unwrap_or_else(|| "0.0".into());
            writeln!(out, "  %{d}.i32 = call i32 @tyra_float_is_nan(double {x})").unwrap();
            writeln!(out, "  %{d} = trunc i32 %{d}.i32 to i1").unwrap();
            true
        }
        "__float_is_infinite" => {
            let d = dest.as_deref().unwrap_or("_float_is_inf");
            let x = args
                .first()
                .map(|a| operand_ref(a, func))
                .unwrap_or_else(|| "0.0".into());
            writeln!(
                out,
                "  %{d}.i32 = call i32 @tyra_float_is_infinite(double {x})"
            )
            .unwrap();
            writeln!(out, "  %{d} = trunc i32 %{d}.i32 to i1").unwrap();
            true
        }
        // §17.3.6 Map<K,V> generic intrinsics (ADR-0015).
        // Names: __map_new__K__V, __map_insert__K__V, __map_contains__K
        // Boxing strategy:
        //   Int  key/val → GC_malloc(8) + store i64
        //   Bool key/val → GC_malloc(8) + store i8 (zero-extended to 8 bytes)
        //   String key/val → GC_malloc(8) + store ptr
        _ if fname.starts_with("__map_new__") => {
            // Parse K and V from the name suffix.
            let suffix = fname.strip_prefix("__map_new__").unwrap_or("");
            let parts: Vec<&str> = suffix.splitn(2, "__").collect();
            let k = parts.first().copied().unwrap_or("String");
            let d = dest.as_deref().unwrap_or("_map");
            // Call tyra_map_new with compiler-emitted eq/hash function addresses.
            writeln!(
                out,
                "  %{d} = call ptr @tyra_map_new(ptr @tyra_eq_{k}, ptr @tyra_hash_{k})"
            )
            .unwrap();
            true
        }
        _ if fname.starts_with("__map_insert__") => {
            let suffix = fname.strip_prefix("__map_insert__").unwrap_or("");
            let parts: Vec<&str> = suffix.splitn(2, "__").collect();
            let k = parts.first().copied().unwrap_or("String");
            let v = parts.get(1).copied().unwrap_or("Int");
            let d = dest.as_deref().unwrap_or("_map_ins");
            let m = args.first().map(|a| operand_ref(a, func)).unwrap_or_else(|| "null".into());
            let k_val = args.get(1).map(|a| operand_ref(a, func)).unwrap_or_else(|| "null".into());
            let v_val = args.get(2).map(|a| operand_ref(a, func)).unwrap_or_else(|| "0".into());
            // Box key.
            emit_box_value(out, &format!("{d}.kbox"), k_val.as_str(), k, d);
            // Box value.
            emit_box_value(out, &format!("{d}.vbox"), v_val.as_str(), v, d);
            writeln!(
                out,
                "  %{d} = call ptr @tyra_map_insert(ptr {m}, ptr %{d}.kbox, ptr %{d}.vbox)"
            )
            .unwrap();
            true
        }
        _ if fname.starts_with("__map_contains__") => {
            let k = fname.strip_prefix("__map_contains__").unwrap_or("String");
            let d = dest.as_deref().unwrap_or("_map_has");
            let m = args.first().map(|a| operand_ref(a, func)).unwrap_or_else(|| "null".into());
            let k_val = args.get(1).map(|a| operand_ref(a, func)).unwrap_or_else(|| "null".into());
            emit_box_value(out, &format!("{d}.kbox"), k_val.as_str(), k, d);
            writeln!(
                out,
                "  %{d}.i32 = call i32 @tyra_map_contains(ptr {m}, ptr %{d}.kbox)"
            )
            .unwrap();
            writeln!(out, "  %{d} = icmp ne i32 %{d}.i32, 0").unwrap();
            true
        }
        _ if fname == "__map_len" => {
            let d = dest.as_deref().unwrap_or("_map_len");
            let m = args.first().map(|a| operand_ref(a, func)).unwrap_or_else(|| "null".into());
            writeln!(out, "  %{d} = call i64 @tyra_map_len(ptr {m})").unwrap();
            true
        }
        // §17.3.5: list stdlib intrinsics (List<Int> only).
        "__list_int_push" => {
            emit_list_int_push(out, dest.as_deref(), args, func, ctx);
            true
        }
        "__list_int_sum" => {
            emit_list_int_sum(out, dest.as_deref(), args, func, ctx);
            true
        }
        "__list_int_contains" => {
            emit_list_int_contains(out, dest.as_deref(), args, func, ctx);
            true
        }
        "__list_int_index_of" => {
            emit_list_int_index_of(out, dest.as_deref(), args, func, ctx);
            true
        }
        "__list_int_max" => {
            emit_list_int_min_max(out, dest.as_deref(), args, func, ctx, true);
            true
        }
        "__list_int_min" => {
            emit_list_int_min_max(out, dest.as_deref(), args, func, ctx, false);
            true
        }
        // §17.3.5 Phase C: list.map / list.filter / list.fold (ADR-0011).
        "__list_map_int" => {
            emit_list_map(out, dest.as_deref(), args, func, ctx, "i64", "List__Int");
            true
        }
        "__list_filter_int" => {
            emit_list_filter(out, dest.as_deref(), args, func, ctx, "i64", "List__Int");
            true
        }
        "__list_fold_int" => {
            emit_list_fold(out, dest.as_deref(), args, func, ctx, "i64");
            true
        }
        "__list_map_str" => {
            emit_list_map(out, dest.as_deref(), args, func, ctx, "ptr", "List__String");
            true
        }
        "__list_filter_str" => {
            emit_list_filter(out, dest.as_deref(), args, func, ctx, "ptr", "List__String");
            true
        }
        "__list_fold_str" => {
            emit_list_fold(out, dest.as_deref(), args, func, ctx, "ptr");
            true
        }
        "__bench_clock_ns" => {
            // §18.8: wall-clock nanoseconds. Calls @__bench_clock_ns() -> i64.
            let d = dest.as_deref().unwrap_or("_bench_ns");
            writeln!(out, "  %{d} = call i64 @__bench_clock_ns()").unwrap();
            true
        }
        "sys__exit" => {
            // §17.1: core.sys.exit(_ code: Int) -> Never
            if let Some(arg) = args.first() {
                let val = operand_ref(arg, func);
                writeln!(
                    out,
                    "  %{}.i32 = trunc i64 {val} to i32",
                    dest.as_deref().unwrap_or("_exit")
                )
                .unwrap();
                writeln!(
                    out,
                    "  call void @exit(i32 %{}.i32)",
                    dest.as_deref().unwrap_or("_exit")
                )
                .unwrap();
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
/// Print "panic at FILE:LINE:\nMESSAGE\n" to stderr, then abort (ADR 0014).
fn emit_panic_call(
    out: &mut String,
    args: &[Operand],
    loc: tyra_mir::SourceLoc,
    func: &Function,
    ctx: &EmitCtx,
) {
    // If we have source location, emit "panic at FILE:LINE:\n" to fd 2 (stderr).
    if !loc.is_dummy() && (loc.file_id as usize) < ctx.source_files.len() {
        let file_id = loc.file_id;
        let line = loc.line;
        writeln!(
            out,
            "  call i32 (i32, ptr, ...) @dprintf(i32 2, ptr @.fmt.panic_loc, ptr @.src.{file_id}, i64 {line})"
        )
        .unwrap();
    }
    // Print the message string followed by a newline to stderr.
    if let Some(arg) = args.first() {
        let val = operand_ref(arg, func);
        writeln!(
            out,
            "  call i32 (i32, ptr, ...) @dprintf(i32 2, ptr @.fmt.str_ln, ptr {val})"
        )
        .unwrap();
    }
    // Write sentinel to stderr so the test runner can confirm this is intentional panic,
    // not sys.exit(101) (ADR 0012: 2-stage identification).
    writeln!(
        out,
        "  call i32 (i32, ptr, ...) @dprintf(i32 2, ptr @.str.panic_sentinel)"
    )
    .unwrap();
    writeln!(out, "  call void @exit(i32 101)").unwrap();
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
    writeln!(
        out,
        "  %{d}.argp = getelementptr ptr, ptr %{d}.argv, i64 %{d}.i"
    )
    .unwrap();
    writeln!(out, "  %{d}.arg = load ptr, ptr %{d}.argp").unwrap();
    writeln!(
        out,
        "  %{d}.dstp = getelementptr ptr, ptr %{d}.data, i64 %{d}.i"
    )
    .unwrap();
    writeln!(out, "  store ptr %{d}.arg, ptr %{d}.dstp").unwrap();
    writeln!(out, "  %{d}.next = add i64 %{d}.i, 1").unwrap();
    writeln!(out, "  store i64 %{d}.next, ptr %{d}.ctr").unwrap();
    writeln!(out, "  br label %{d}.loop").unwrap();
    writeln!(out, "{d}.end:").unwrap();
    // Build List struct {ptr, i64}
    writeln!(
        out,
        "  %{d}.s0 = insertvalue {list_ty} undef, ptr %{d}.data, 0"
    )
    .unwrap();
    writeln!(
        out,
        "  %{d} = insertvalue {list_ty} %{d}.s0, i64 %{d}.argc64, 1"
    )
    .unwrap();
}

/// M10 phase 1: `__fs_read_raw(path: String) -> String`.
/// Delegates to `@tyra_fs_read` in the runtime. Returns empty C string on
/// error; the caller is expected to check `__fs_errno()` to discriminate.
fn emit_fs_read_raw(out: &mut String, dest: Option<&str>, args: &[Operand], func: &Function) {
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
    writeln!(
        out,
        "  call void @tyra_fs_write(ptr {path}, ptr {contents})"
    )
    .unwrap();
}

/// M10 phase 1b: `__fs_exists(path: String) -> Bool`.
/// Delegates to `@tyra_fs_exists`; truncates the i32 return to i1.
fn emit_fs_exists(out: &mut String, dest: Option<&str>, args: &[Operand], func: &Function) {
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
    writeln!(
        out,
        "  %{d} = call i64 @tyra_json_get(i64 {handle}, ptr {key})"
    )
    .unwrap();
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
    writeln!(
        out,
        "  %{d} = call i64 @tyra_json_at(i64 {handle}, i64 {index})"
    )
    .unwrap();
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
fn emit_http_server_route(out: &mut String, dest: Option<&str>, args: &[Operand], func: &Function) {
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

// ---------------------------------------------------------------------------
// §17.3.4: string stdlib emit helpers. All operate on ptr-typed String
// operands and either return a primitive (i64/bool) or a new ptr.
// ---------------------------------------------------------------------------

fn emit_string_ptr_to_i64(
    out: &mut String,
    dest: Option<&str>,
    callee: &str,
    args: &[Operand],
    func: &Function,
) {
    let d = dest.unwrap_or("_string");
    let s = args
        .first()
        .map(|a| operand_ref(a, func))
        .unwrap_or_else(|| "null".into());
    writeln!(out, "  %{d} = call i64 @{callee}(ptr {s})").unwrap();
}

fn emit_string_ptr_to_ptr(
    out: &mut String,
    dest: Option<&str>,
    callee: &str,
    args: &[Operand],
    func: &Function,
) {
    let d = dest.unwrap_or("_string");
    let s = args
        .first()
        .map(|a| operand_ref(a, func))
        .unwrap_or_else(|| "null".into());
    writeln!(out, "  %{d} = call ptr @{callee}(ptr {s})").unwrap();
}

fn emit_string_ptr_to_bool(
    out: &mut String,
    dest: Option<&str>,
    callee: &str,
    args: &[Operand],
    func: &Function,
) {
    let d = dest.unwrap_or("_string");
    let s = args
        .first()
        .map(|a| operand_ref(a, func))
        .unwrap_or_else(|| "null".into());
    writeln!(out, "  %{d}.i32 = call i32 @{callee}(ptr {s})").unwrap();
    writeln!(out, "  %{d} = icmp ne i32 %{d}.i32, 0").unwrap();
}

fn emit_string_ptr2_to_bool(
    out: &mut String,
    dest: Option<&str>,
    callee: &str,
    args: &[Operand],
    func: &Function,
) {
    let d = dest.unwrap_or("_string");
    let a = args
        .first()
        .map(|x| operand_ref(x, func))
        .unwrap_or_else(|| "null".into());
    let b = args
        .get(1)
        .map(|x| operand_ref(x, func))
        .unwrap_or_else(|| "null".into());
    writeln!(out, "  %{d}.i32 = call i32 @{callee}(ptr {a}, ptr {b})").unwrap();
    writeln!(out, "  %{d} = icmp ne i32 %{d}.i32, 0").unwrap();
}

fn emit_string_ptr_i64_to_i64(
    out: &mut String,
    dest: Option<&str>,
    callee: &str,
    args: &[Operand],
    func: &Function,
) {
    let d = dest.unwrap_or("_string");
    let s = args
        .first()
        .map(|a| operand_ref(a, func))
        .unwrap_or_else(|| "null".into());
    let n = args
        .get(1)
        .map(|a| operand_ref(a, func))
        .unwrap_or_else(|| "0".into());
    writeln!(out, "  %{d} = call i64 @{callee}(ptr {s}, i64 {n})").unwrap();
}

fn emit_string_ptr_i64_i64_to_ptr(
    out: &mut String,
    dest: Option<&str>,
    callee: &str,
    args: &[Operand],
    func: &Function,
) {
    let d = dest.unwrap_or("_string");
    let s = args
        .first()
        .map(|a| operand_ref(a, func))
        .unwrap_or_else(|| "null".into());
    let lo = args
        .get(1)
        .map(|a| operand_ref(a, func))
        .unwrap_or_else(|| "0".into());
    let hi = args
        .get(2)
        .map(|a| operand_ref(a, func))
        .unwrap_or_else(|| "0".into());
    writeln!(
        out,
        "  %{d} = call ptr @{callee}(ptr {s}, i64 {lo}, i64 {hi})"
    )
    .unwrap();
}

fn emit_string_i64_to_ptr(
    out: &mut String,
    dest: Option<&str>,
    callee: &str,
    args: &[Operand],
    func: &Function,
) {
    let d = dest.unwrap_or("_string");
    let n = args
        .first()
        .map(|a| operand_ref(a, func))
        .unwrap_or_else(|| "0".into());
    writeln!(out, "  %{d} = call ptr @{callee}(i64 {n})").unwrap();
}

/// `__string_split_whitespace(s) -> List<String>`. Allocates a 16-byte
/// stack slot for the {ptr, i64} result, hands it to the runtime by
/// reference, then loads back the populated struct.
fn emit_string_split_ws(
    out: &mut String,
    dest: Option<&str>,
    args: &[Operand],
    func: &Function,
    ctx: &EmitCtx,
) {
    let d = dest.unwrap_or("_split_ws");
    let s = args
        .first()
        .map(|a| operand_ref(a, func))
        .unwrap_or_else(|| "null".into());
    let list_ty = if let Some(info) = ctx.struct_map.get("List__String") {
        info.llvm_name.clone()
    } else {
        "%struct.List__String".into()
    };
    writeln!(out, "  %{d}.slot = alloca {list_ty}").unwrap();
    writeln!(
        out,
        "  call void @tyra_string_split_whitespace(ptr {s}, ptr %{d}.slot)"
    )
    .unwrap();
    writeln!(out, "  %{d} = load {list_ty}, ptr %{d}.slot").unwrap();
}

/// `__string_split(s, sep) -> List<String>`. Same out-parameter shape
/// as `__string_split_whitespace`.
fn emit_string_split(
    out: &mut String,
    dest: Option<&str>,
    args: &[Operand],
    func: &Function,
    ctx: &EmitCtx,
) {
    let d = dest.unwrap_or("_split");
    let s = args
        .first()
        .map(|a| operand_ref(a, func))
        .unwrap_or_else(|| "null".into());
    let sep = args
        .get(1)
        .map(|a| operand_ref(a, func))
        .unwrap_or_else(|| "null".into());
    let list_ty = if let Some(info) = ctx.struct_map.get("List__String") {
        info.llvm_name.clone()
    } else {
        "%struct.List__String".into()
    };
    writeln!(out, "  %{d}.slot = alloca {list_ty}").unwrap();
    writeln!(
        out,
        "  call void @tyra_string_split(ptr {s}, ptr {sep}, ptr %{d}.slot)"
    )
    .unwrap();
    writeln!(out, "  %{d} = load {list_ty}, ptr %{d}.slot").unwrap();
}

/// `__string_replace(s, from, to) -> String` — three-ptr-to-ptr call.
fn emit_string_replace(
    out: &mut String,
    dest: Option<&str>,
    args: &[Operand],
    func: &Function,
) {
    let d = dest.unwrap_or("_replace");
    let s = args
        .first()
        .map(|a| operand_ref(a, func))
        .unwrap_or_else(|| "null".into());
    let from = args
        .get(1)
        .map(|a| operand_ref(a, func))
        .unwrap_or_else(|| "null".into());
    let to = args
        .get(2)
        .map(|a| operand_ref(a, func))
        .unwrap_or_else(|| "null".into());
    writeln!(
        out,
        "  %{d} = call ptr @tyra_string_replace(ptr {s}, ptr {from}, ptr {to})"
    )
    .unwrap();
}

/// `__string_join(parts, sep) -> String` — passes the `List<String>` struct
/// by alloca'd pointer (same pattern as `__string_split` out-param, but in
/// reverse: we store the struct value into a slot and pass a pointer to the
/// runtime, which reads the `{ptr, i64}` layout directly).
fn emit_string_join(
    out: &mut String,
    dest: Option<&str>,
    args: &[Operand],
    func: &Function,
    ctx: &EmitCtx,
) {
    let d = dest.unwrap_or("_join");
    let list_val = args
        .first()
        .map(|a| operand_ref(a, func))
        .unwrap_or_else(|| "undef".into());
    let sep = args
        .get(1)
        .map(|a| operand_ref(a, func))
        .unwrap_or_else(|| "null".into());
    let list_ty = if let Some(info) = ctx.struct_map.get("List__String") {
        info.llvm_name.clone()
    } else {
        "%struct.List__String".into()
    };
    writeln!(out, "  %{d}.lslot = alloca {list_ty}").unwrap();
    writeln!(out, "  store {list_ty} {list_val}, ptr %{d}.lslot").unwrap();
    writeln!(
        out,
        "  %{d} = call ptr @tyra_string_join(ptr %{d}.lslot, ptr {sep})"
    )
    .unwrap();
}

// ---------------------------------------------------------------------------
// §17.3.x: float stdlib emit helpers.
// ---------------------------------------------------------------------------

/// `__float_eq(a, b) -> Bool`.
fn emit_float_double2_to_bool(
    out: &mut String,
    dest: Option<&str>,
    callee: &str,
    args: &[Operand],
    func: &Function,
) {
    let d = dest.unwrap_or("_float");
    let a = args
        .first()
        .map(|x| operand_ref(x, func))
        .unwrap_or_else(|| "0.0".into());
    let b = args
        .get(1)
        .map(|x| operand_ref(x, func))
        .unwrap_or_else(|| "0.0".into());
    writeln!(
        out,
        "  %{d}.i32 = call i32 @{callee}(double {a}, double {b})"
    )
    .unwrap();
    writeln!(out, "  %{d} = icmp ne i32 %{d}.i32, 0").unwrap();
}

/// `__float_approx_eq(a, b, eps) -> Bool` — three double args.
fn emit_float_approx_eq(out: &mut String, dest: Option<&str>, args: &[Operand], func: &Function) {
    let d = dest.unwrap_or("_float_approx");
    let a = args
        .first()
        .map(|x| operand_ref(x, func))
        .unwrap_or_else(|| "0.0".into());
    let b = args
        .get(1)
        .map(|x| operand_ref(x, func))
        .unwrap_or_else(|| "0.0".into());
    let eps = args
        .get(2)
        .map(|x| operand_ref(x, func))
        .unwrap_or_else(|| "0.0".into());
    writeln!(
        out,
        "  %{d}.i32 = call i32 @tyra_float_approx_eq(double {a}, double {b}, double {eps})"
    )
    .unwrap();
    writeln!(out, "  %{d} = icmp ne i32 %{d}.i32, 0").unwrap();
}

/// `__float_abs(x) -> Float`, `__float_floor`, `__float_ceil`, `__float_round`.
fn emit_float_double_to_double(
    out: &mut String,
    dest: Option<&str>,
    callee: &str,
    args: &[Operand],
    func: &Function,
) {
    let d = dest.unwrap_or("_float");
    let x = args
        .first()
        .map(|a| operand_ref(a, func))
        .unwrap_or_else(|| "0.0".into());
    writeln!(out, "  %{d} = call double @{callee}(double {x})").unwrap();
}

/// `__float_min(a, b) -> Float`, `__float_max(a, b) -> Float`.
fn emit_float_double2_to_double(
    out: &mut String,
    dest: Option<&str>,
    callee: &str,
    args: &[Operand],
    func: &Function,
) {
    let d = dest.unwrap_or("_float");
    let a = args
        .first()
        .map(|x| operand_ref(x, func))
        .unwrap_or_else(|| "0.0".into());
    let b = args
        .get(1)
        .map(|x| operand_ref(x, func))
        .unwrap_or_else(|| "0.0".into());
    writeln!(
        out,
        "  %{d} = call double @{callee}(double {a}, double {b})"
    )
    .unwrap();
}

/// `__float_to_string(x) -> String`.
fn emit_float_to_string(out: &mut String, dest: Option<&str>, args: &[Operand], func: &Function) {
    let d = dest.unwrap_or("_float_str");
    let x = args
        .first()
        .map(|a| operand_ref(a, func))
        .unwrap_or_else(|| "0.0".into());
    writeln!(out, "  %{d} = call ptr @tyra_float_to_string(double {x})").unwrap();
}

/// `__float_parse(s) -> Float`.
fn emit_float_parse(out: &mut String, dest: Option<&str>, args: &[Operand], func: &Function) {
    let d = dest.unwrap_or("_float_parse");
    let s = args
        .first()
        .map(|a| operand_ref(a, func))
        .unwrap_or_else(|| "null".into());
    writeln!(out, "  %{d} = call double @tyra_float_parse(ptr {s})").unwrap();
}

/// `__float_from_int(n) -> Float`.
fn emit_float_from_int(out: &mut String, dest: Option<&str>, args: &[Operand], func: &Function) {
    let d = dest.unwrap_or("_float_from_int");
    let n = args
        .first()
        .map(|a| operand_ref(a, func))
        .unwrap_or_else(|| "0".into());
    writeln!(out, "  %{d} = call double @tyra_float_from_int(i64 {n})").unwrap();
}

/// `__float_to_int(x) -> Int`.
fn emit_float_to_int(out: &mut String, dest: Option<&str>, args: &[Operand], func: &Function) {
    let d = dest.unwrap_or("_float_to_int");
    let x = args
        .first()
        .map(|a| operand_ref(a, func))
        .unwrap_or_else(|| "0.0".into());
    writeln!(out, "  %{d} = call i64 @tyra_float_to_int(double {x})").unwrap();
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
    writeln!(
        out,
        "  %{d}.val = call i64 @strtol(ptr {val}, ptr %{d}.endp, i32 10)"
    )
    .unwrap();
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
    writeln!(
        out,
        "  %{d}.some.v = insertvalue {opt_ty} %{d}.some.s0, i64 %{d}.val, 1"
    )
    .unwrap();
    writeln!(out, "  store {opt_ty} %{d}.some.v, ptr %{d}.slot").unwrap();
    writeln!(out, "  br label %{d}.merge").unwrap();
    // None path
    writeln!(out, "{d}.none:").unwrap();
    writeln!(out, "  %{d}.none.s0 = insertvalue {opt_ty} undef, i8 1, 0").unwrap();
    writeln!(
        out,
        "  %{d}.none.v = insertvalue {opt_ty} %{d}.none.s0, i64 0, 1"
    )
    .unwrap();
    writeln!(out, "  store {opt_ty} %{d}.none.v, ptr %{d}.slot").unwrap();
    writeln!(out, "  br label %{d}.merge").unwrap();
    // Merge
    writeln!(out, "{d}.merge:").unwrap();
    writeln!(out, "  %{d} = load {opt_ty}, ptr %{d}.slot").unwrap();
}

// ---------------------------------------------------------------------------
// §17.3.5: list stdlib emit helpers. All operate on `List<Int>` values laid
// out as `{ptr data, i64 len}`. Implemented purely in LLVM IR — no runtime
// C ABI — because the list layout is compiler-owned. Every operation is
// immutable: mutating inputs is never allowed; callers always receive a
// fresh GC-allocated buffer when the output is a list.
// ---------------------------------------------------------------------------

/// Look up the LLVM struct type name for `List<Int>`. All `List<Int>` values
/// share one physical layout; the fallback keeps emission well-formed when
/// the monomorphized struct has not been registered (e.g. intrinsic called
/// from a context where no List<Int> literal forced monomorphization).
fn list_int_llvm_ty(ctx: &EmitCtx) -> String {
    if let Some(info) = ctx.struct_map.get("List__Int") {
        info.llvm_name.clone()
    } else {
        "%struct.List__Int".into()
    }
}

fn option_int_llvm_ty(ctx: &EmitCtx) -> String {
    if let Some(info) = ctx.struct_map.get("Option__Int") {
        info.llvm_name.clone()
    } else {
        "%struct.Option__Int".into()
    }
}

/// Box a value into a GC_malloc'd 8-byte slot and return the slot as `ptr`.
/// `box_dest` is the name for the alloca/slot temp (without `%`).
/// `val_ref` is the LLVM value reference (e.g. `%_t3` or `42`).
/// `ty_name` is the Tyra type name: "Int", "Bool", "String".
/// `unique_prefix` is used to avoid name clashes for multiple boxes in one BB.
fn emit_box_value(out: &mut String, box_dest: &str, val_ref: &str, ty_name: &str, unique_prefix: &str) {
    let _ = unique_prefix; // currently unused; kept for future disambiguation
    match ty_name {
        "Int" => {
            writeln!(out, "  %{box_dest} = call ptr @GC_malloc(i64 8)").unwrap();
            writeln!(out, "  store i64 {val_ref}, ptr %{box_dest}").unwrap();
        }
        "Bool" => {
            writeln!(out, "  %{box_dest} = call ptr @GC_malloc(i64 8)").unwrap();
            // Bool is i1 in LLVM; zero-extend to i8 before storing.
            writeln!(out, "  %{box_dest}.i8 = zext i1 {val_ref} to i8").unwrap();
            writeln!(out, "  store i8 %{box_dest}.i8, ptr %{box_dest}").unwrap();
        }
        "String" | _ => {
            // String (and unknown types): the value is already a ptr; store ptr.
            writeln!(out, "  %{box_dest} = call ptr @GC_malloc(i64 8)").unwrap();
            writeln!(out, "  store ptr {val_ref}, ptr %{box_dest}").unwrap();
        }
    }
}

/// `__list_int_push(list, x)` — immutable append. Allocates a new buffer of
/// size `(len + 1) * 8`, copies every element, writes `x` at index `len`, and
/// returns a fresh `{ptr, i64}` struct. Input is never mutated.
fn emit_list_int_push(
    out: &mut String,
    dest: Option<&str>,
    args: &[Operand],
    func: &Function,
    ctx: &EmitCtx,
) {
    let d = dest.unwrap_or("_list_push");
    let list_ty = list_int_llvm_ty(ctx);
    let list_val = args
        .first()
        .map(|a| operand_ref(a, func))
        .unwrap_or_else(|| "undef".into());
    let x = args
        .get(1)
        .map(|a| operand_ref(a, func))
        .unwrap_or_else(|| "0".into());

    writeln!(out, "  %{d}.olddata = extractvalue {list_ty} {list_val}, 0").unwrap();
    writeln!(out, "  %{d}.oldlen = extractvalue {list_ty} {list_val}, 1").unwrap();
    writeln!(out, "  %{d}.newlen = add i64 %{d}.oldlen, 1").unwrap();
    writeln!(out, "  %{d}.size = mul i64 %{d}.newlen, 8").unwrap();
    writeln!(out, "  %{d}.newdata = call ptr @GC_malloc(i64 %{d}.size)").unwrap();
    writeln!(out, "  %{d}.null = icmp eq ptr %{d}.newdata, null").unwrap();
    writeln!(out, "  br i1 %{d}.null, label %{d}.oom, label %{d}.copy").unwrap();
    writeln!(out, "{d}.oom:").unwrap();
    writeln!(out, "  call void @abort()").unwrap();
    writeln!(out, "  unreachable").unwrap();
    writeln!(out, "{d}.copy:").unwrap();
    writeln!(out, "  %{d}.ctr = alloca i64").unwrap();
    writeln!(out, "  store i64 0, ptr %{d}.ctr").unwrap();
    writeln!(out, "  br label %{d}.loop").unwrap();
    writeln!(out, "{d}.loop:").unwrap();
    writeln!(out, "  %{d}.i = load i64, ptr %{d}.ctr").unwrap();
    writeln!(out, "  %{d}.done = icmp sge i64 %{d}.i, %{d}.oldlen").unwrap();
    writeln!(out, "  br i1 %{d}.done, label %{d}.tail, label %{d}.body").unwrap();
    writeln!(out, "{d}.body:").unwrap();
    writeln!(
        out,
        "  %{d}.srcp = getelementptr i64, ptr %{d}.olddata, i64 %{d}.i"
    )
    .unwrap();
    writeln!(out, "  %{d}.v = load i64, ptr %{d}.srcp").unwrap();
    writeln!(
        out,
        "  %{d}.dstp = getelementptr i64, ptr %{d}.newdata, i64 %{d}.i"
    )
    .unwrap();
    writeln!(out, "  store i64 %{d}.v, ptr %{d}.dstp").unwrap();
    writeln!(out, "  %{d}.next = add i64 %{d}.i, 1").unwrap();
    writeln!(out, "  store i64 %{d}.next, ptr %{d}.ctr").unwrap();
    writeln!(out, "  br label %{d}.loop").unwrap();
    writeln!(out, "{d}.tail:").unwrap();
    writeln!(
        out,
        "  %{d}.tailp = getelementptr i64, ptr %{d}.newdata, i64 %{d}.oldlen"
    )
    .unwrap();
    writeln!(out, "  store i64 {x}, ptr %{d}.tailp").unwrap();
    writeln!(
        out,
        "  %{d}.s0 = insertvalue {list_ty} undef, ptr %{d}.newdata, 0"
    )
    .unwrap();
    writeln!(
        out,
        "  %{d} = insertvalue {list_ty} %{d}.s0, i64 %{d}.newlen, 1"
    )
    .unwrap();
}

/// `__list_int_sum(list)` — fold with `+`, identity `0`.
fn emit_list_int_sum(
    out: &mut String,
    dest: Option<&str>,
    args: &[Operand],
    func: &Function,
    ctx: &EmitCtx,
) {
    let d = dest.unwrap_or("_list_sum");
    let list_ty = list_int_llvm_ty(ctx);
    let list_val = args
        .first()
        .map(|a| operand_ref(a, func))
        .unwrap_or_else(|| "undef".into());
    writeln!(out, "  %{d}.data = extractvalue {list_ty} {list_val}, 0").unwrap();
    writeln!(out, "  %{d}.len = extractvalue {list_ty} {list_val}, 1").unwrap();
    writeln!(out, "  %{d}.acc = alloca i64").unwrap();
    writeln!(out, "  store i64 0, ptr %{d}.acc").unwrap();
    writeln!(out, "  %{d}.ctr = alloca i64").unwrap();
    writeln!(out, "  store i64 0, ptr %{d}.ctr").unwrap();
    writeln!(out, "  br label %{d}.loop").unwrap();
    writeln!(out, "{d}.loop:").unwrap();
    writeln!(out, "  %{d}.i = load i64, ptr %{d}.ctr").unwrap();
    writeln!(out, "  %{d}.done = icmp sge i64 %{d}.i, %{d}.len").unwrap();
    writeln!(out, "  br i1 %{d}.done, label %{d}.end, label %{d}.body").unwrap();
    writeln!(out, "{d}.body:").unwrap();
    writeln!(
        out,
        "  %{d}.p = getelementptr i64, ptr %{d}.data, i64 %{d}.i"
    )
    .unwrap();
    writeln!(out, "  %{d}.v = load i64, ptr %{d}.p").unwrap();
    writeln!(out, "  %{d}.cur = load i64, ptr %{d}.acc").unwrap();
    writeln!(out, "  %{d}.sum = add i64 %{d}.cur, %{d}.v").unwrap();
    writeln!(out, "  store i64 %{d}.sum, ptr %{d}.acc").unwrap();
    writeln!(out, "  %{d}.next = add i64 %{d}.i, 1").unwrap();
    writeln!(out, "  store i64 %{d}.next, ptr %{d}.ctr").unwrap();
    writeln!(out, "  br label %{d}.loop").unwrap();
    writeln!(out, "{d}.end:").unwrap();
    writeln!(out, "  %{d} = load i64, ptr %{d}.acc").unwrap();
}

/// `__list_int_contains(list, x)` — linear search; returns i1.
fn emit_list_int_contains(
    out: &mut String,
    dest: Option<&str>,
    args: &[Operand],
    func: &Function,
    ctx: &EmitCtx,
) {
    let d = dest.unwrap_or("_list_contains");
    let list_ty = list_int_llvm_ty(ctx);
    let list_val = args
        .first()
        .map(|a| operand_ref(a, func))
        .unwrap_or_else(|| "undef".into());
    let x = args
        .get(1)
        .map(|a| operand_ref(a, func))
        .unwrap_or_else(|| "0".into());
    writeln!(out, "  %{d}.data = extractvalue {list_ty} {list_val}, 0").unwrap();
    writeln!(out, "  %{d}.len = extractvalue {list_ty} {list_val}, 1").unwrap();
    writeln!(out, "  %{d}.res = alloca i1").unwrap();
    writeln!(out, "  store i1 0, ptr %{d}.res").unwrap();
    writeln!(out, "  %{d}.ctr = alloca i64").unwrap();
    writeln!(out, "  store i64 0, ptr %{d}.ctr").unwrap();
    writeln!(out, "  br label %{d}.loop").unwrap();
    writeln!(out, "{d}.loop:").unwrap();
    writeln!(out, "  %{d}.i = load i64, ptr %{d}.ctr").unwrap();
    writeln!(out, "  %{d}.done = icmp sge i64 %{d}.i, %{d}.len").unwrap();
    writeln!(out, "  br i1 %{d}.done, label %{d}.end, label %{d}.body").unwrap();
    writeln!(out, "{d}.body:").unwrap();
    writeln!(
        out,
        "  %{d}.p = getelementptr i64, ptr %{d}.data, i64 %{d}.i"
    )
    .unwrap();
    writeln!(out, "  %{d}.v = load i64, ptr %{d}.p").unwrap();
    writeln!(out, "  %{d}.eq = icmp eq i64 %{d}.v, {x}").unwrap();
    writeln!(out, "  br i1 %{d}.eq, label %{d}.hit, label %{d}.cont").unwrap();
    writeln!(out, "{d}.hit:").unwrap();
    writeln!(out, "  store i1 1, ptr %{d}.res").unwrap();
    writeln!(out, "  br label %{d}.end").unwrap();
    writeln!(out, "{d}.cont:").unwrap();
    writeln!(out, "  %{d}.next = add i64 %{d}.i, 1").unwrap();
    writeln!(out, "  store i64 %{d}.next, ptr %{d}.ctr").unwrap();
    writeln!(out, "  br label %{d}.loop").unwrap();
    writeln!(out, "{d}.end:").unwrap();
    writeln!(out, "  %{d} = load i1, ptr %{d}.res").unwrap();
}

/// `__list_int_index_of(list, x)` — first-match linear search; returns
/// `Some(i)` for the lowest match, or `None` if absent.
fn emit_list_int_index_of(
    out: &mut String,
    dest: Option<&str>,
    args: &[Operand],
    func: &Function,
    ctx: &EmitCtx,
) {
    let d = dest.unwrap_or("_list_index_of");
    let list_ty = list_int_llvm_ty(ctx);
    let opt_ty = option_int_llvm_ty(ctx);
    let list_val = args
        .first()
        .map(|a| operand_ref(a, func))
        .unwrap_or_else(|| "undef".into());
    let x = args
        .get(1)
        .map(|a| operand_ref(a, func))
        .unwrap_or_else(|| "0".into());
    writeln!(out, "  %{d}.data = extractvalue {list_ty} {list_val}, 0").unwrap();
    writeln!(out, "  %{d}.len = extractvalue {list_ty} {list_val}, 1").unwrap();
    writeln!(out, "  %{d}.slot = alloca {opt_ty}").unwrap();
    // Initialize to None so the post-loop fallthrough path is correct.
    writeln!(out, "  %{d}.none0 = insertvalue {opt_ty} undef, i8 1, 0").unwrap();
    writeln!(
        out,
        "  %{d}.none1 = insertvalue {opt_ty} %{d}.none0, i64 0, 1"
    )
    .unwrap();
    writeln!(out, "  store {opt_ty} %{d}.none1, ptr %{d}.slot").unwrap();
    writeln!(out, "  %{d}.ctr = alloca i64").unwrap();
    writeln!(out, "  store i64 0, ptr %{d}.ctr").unwrap();
    writeln!(out, "  br label %{d}.loop").unwrap();
    writeln!(out, "{d}.loop:").unwrap();
    writeln!(out, "  %{d}.i = load i64, ptr %{d}.ctr").unwrap();
    writeln!(out, "  %{d}.done = icmp sge i64 %{d}.i, %{d}.len").unwrap();
    writeln!(out, "  br i1 %{d}.done, label %{d}.end, label %{d}.body").unwrap();
    writeln!(out, "{d}.body:").unwrap();
    writeln!(
        out,
        "  %{d}.p = getelementptr i64, ptr %{d}.data, i64 %{d}.i"
    )
    .unwrap();
    writeln!(out, "  %{d}.v = load i64, ptr %{d}.p").unwrap();
    writeln!(out, "  %{d}.eq = icmp eq i64 %{d}.v, {x}").unwrap();
    writeln!(out, "  br i1 %{d}.eq, label %{d}.hit, label %{d}.cont").unwrap();
    writeln!(out, "{d}.hit:").unwrap();
    writeln!(out, "  %{d}.some0 = insertvalue {opt_ty} undef, i8 0, 0").unwrap();
    writeln!(
        out,
        "  %{d}.some1 = insertvalue {opt_ty} %{d}.some0, i64 %{d}.i, 1"
    )
    .unwrap();
    writeln!(out, "  store {opt_ty} %{d}.some1, ptr %{d}.slot").unwrap();
    writeln!(out, "  br label %{d}.end").unwrap();
    writeln!(out, "{d}.cont:").unwrap();
    writeln!(out, "  %{d}.next = add i64 %{d}.i, 1").unwrap();
    writeln!(out, "  store i64 %{d}.next, ptr %{d}.ctr").unwrap();
    writeln!(out, "  br label %{d}.loop").unwrap();
    writeln!(out, "{d}.end:").unwrap();
    writeln!(out, "  %{d} = load {opt_ty}, ptr %{d}.slot").unwrap();
}

/// `__list_int_max(list)` / `__list_int_min(list)` — returns `Some(v)` with
/// the extremum, or `None` for empty lists. `is_max = true` selects max.
fn emit_list_int_min_max(
    out: &mut String,
    dest: Option<&str>,
    args: &[Operand],
    func: &Function,
    ctx: &EmitCtx,
    is_max: bool,
) {
    let d = dest.unwrap_or(if is_max { "_list_max" } else { "_list_min" });
    let list_ty = list_int_llvm_ty(ctx);
    let opt_ty = option_int_llvm_ty(ctx);
    let cmp = if is_max { "sgt" } else { "slt" };
    let list_val = args
        .first()
        .map(|a| operand_ref(a, func))
        .unwrap_or_else(|| "undef".into());
    writeln!(out, "  %{d}.data = extractvalue {list_ty} {list_val}, 0").unwrap();
    writeln!(out, "  %{d}.len = extractvalue {list_ty} {list_val}, 1").unwrap();
    writeln!(out, "  %{d}.slot = alloca {opt_ty}").unwrap();
    writeln!(out, "  %{d}.empty = icmp eq i64 %{d}.len, 0").unwrap();
    writeln!(out, "  br i1 %{d}.empty, label %{d}.none, label %{d}.init").unwrap();
    writeln!(out, "{d}.none:").unwrap();
    writeln!(out, "  %{d}.none0 = insertvalue {opt_ty} undef, i8 1, 0").unwrap();
    writeln!(
        out,
        "  %{d}.none1 = insertvalue {opt_ty} %{d}.none0, i64 0, 1"
    )
    .unwrap();
    writeln!(out, "  store {opt_ty} %{d}.none1, ptr %{d}.slot").unwrap();
    writeln!(out, "  br label %{d}.end").unwrap();
    writeln!(out, "{d}.init:").unwrap();
    writeln!(out, "  %{d}.p0 = getelementptr i64, ptr %{d}.data, i64 0").unwrap();
    writeln!(out, "  %{d}.v0 = load i64, ptr %{d}.p0").unwrap();
    writeln!(out, "  %{d}.best = alloca i64").unwrap();
    writeln!(out, "  store i64 %{d}.v0, ptr %{d}.best").unwrap();
    writeln!(out, "  %{d}.ctr = alloca i64").unwrap();
    writeln!(out, "  store i64 1, ptr %{d}.ctr").unwrap();
    writeln!(out, "  br label %{d}.loop").unwrap();
    writeln!(out, "{d}.loop:").unwrap();
    writeln!(out, "  %{d}.i = load i64, ptr %{d}.ctr").unwrap();
    writeln!(out, "  %{d}.done = icmp sge i64 %{d}.i, %{d}.len").unwrap();
    writeln!(out, "  br i1 %{d}.done, label %{d}.some, label %{d}.body").unwrap();
    writeln!(out, "{d}.body:").unwrap();
    writeln!(
        out,
        "  %{d}.p = getelementptr i64, ptr %{d}.data, i64 %{d}.i"
    )
    .unwrap();
    writeln!(out, "  %{d}.v = load i64, ptr %{d}.p").unwrap();
    writeln!(out, "  %{d}.cur = load i64, ptr %{d}.best").unwrap();
    writeln!(out, "  %{d}.better = icmp {cmp} i64 %{d}.v, %{d}.cur").unwrap();
    writeln!(out, "  br i1 %{d}.better, label %{d}.upd, label %{d}.cont").unwrap();
    writeln!(out, "{d}.upd:").unwrap();
    writeln!(out, "  store i64 %{d}.v, ptr %{d}.best").unwrap();
    writeln!(out, "  br label %{d}.cont").unwrap();
    writeln!(out, "{d}.cont:").unwrap();
    writeln!(out, "  %{d}.next = add i64 %{d}.i, 1").unwrap();
    writeln!(out, "  store i64 %{d}.next, ptr %{d}.ctr").unwrap();
    writeln!(out, "  br label %{d}.loop").unwrap();
    writeln!(out, "{d}.some:").unwrap();
    writeln!(out, "  %{d}.final = load i64, ptr %{d}.best").unwrap();
    writeln!(out, "  %{d}.some0 = insertvalue {opt_ty} undef, i8 0, 0").unwrap();
    writeln!(
        out,
        "  %{d}.some1 = insertvalue {opt_ty} %{d}.some0, i64 %{d}.final, 1"
    )
    .unwrap();
    writeln!(out, "  store {opt_ty} %{d}.some1, ptr %{d}.slot").unwrap();
    writeln!(out, "  br label %{d}.end").unwrap();
    writeln!(out, "{d}.end:").unwrap();
    writeln!(out, "  %{d} = load {opt_ty}, ptr %{d}.slot").unwrap();
}

// ── Phase C helpers ────────────────────────────────────────────────────

/// Emit `getelementptr` + `load` for both fields of a closure fat pointer.
/// After this, `%{pfx}.fnp` holds the function pointer and `%{pfx}.envp`
/// holds the environment pointer, both as `ptr`.
fn emit_fat_ptr_load(out: &mut String, pfx: &str, fat_val: &str) {
    writeln!(
        out,
        "  %{pfx}.fnp_gep = getelementptr %struct.__closure_fat, ptr {fat_val}, i32 0, i32 0"
    )
    .unwrap();
    writeln!(out, "  %{pfx}.fnp = load ptr, ptr %{pfx}.fnp_gep").unwrap();
    writeln!(
        out,
        "  %{pfx}.envp_gep = getelementptr %struct.__closure_fat, ptr {fat_val}, i32 0, i32 1"
    )
    .unwrap();
    writeln!(out, "  %{pfx}.envp = load ptr, ptr %{pfx}.envp_gep").unwrap();
}

fn list_str_llvm_ty(ctx: &EmitCtx) -> String {
    if let Some(info) = ctx.struct_map.get("List__String") {
        info.llvm_name.clone()
    } else {
        "%struct.List__String".into()
    }
}

/// `__list_map_int(xs, f)` / `__list_map_str(xs, f)` — apply closure to every
/// element and return a new list of the same length.
///
/// `elem_ty`: LLVM type of each element (`"i64"` or `"ptr"`).
/// `list_struct`: key used to look up the LLVM struct name (`"List__Int"` or `"List__String"`).
fn emit_list_map(
    out: &mut String,
    dest: Option<&str>,
    args: &[Operand],
    func: &Function,
    ctx: &EmitCtx,
    elem_ty: &str,
    list_struct: &str,
) {
    let d = dest.unwrap_or("_lmap");
    let list_llvm_ty = if list_struct == "List__Int" {
        list_int_llvm_ty(ctx)
    } else {
        list_str_llvm_ty(ctx)
    };
    let list_val = args
        .first()
        .map(|a| operand_ref(a, func))
        .unwrap_or_else(|| "undef".into());
    let fat_val = args
        .get(1)
        .map(|a| operand_ref(a, func))
        .unwrap_or_else(|| "null".into());

    writeln!(
        out,
        "  %{d}.data = extractvalue {list_llvm_ty} {list_val}, 0"
    )
    .unwrap();
    writeln!(
        out,
        "  %{d}.len = extractvalue {list_llvm_ty} {list_val}, 1"
    )
    .unwrap();
    writeln!(out, "  %{d}.size = mul i64 %{d}.len, 8").unwrap();
    writeln!(out, "  %{d}.newdata = call ptr @GC_malloc(i64 %{d}.size)").unwrap();
    writeln!(out, "  %{d}.null = icmp eq ptr %{d}.newdata, null").unwrap();
    writeln!(out, "  br i1 %{d}.null, label %{d}.oom, label %{d}.setup").unwrap();
    writeln!(out, "{d}.oom:").unwrap();
    writeln!(out, "  call void @abort()").unwrap();
    writeln!(out, "  unreachable").unwrap();
    writeln!(out, "{d}.setup:").unwrap();
    emit_fat_ptr_load(out, d, &fat_val);
    writeln!(out, "  %{d}.ctr = alloca i64").unwrap();
    writeln!(out, "  store i64 0, ptr %{d}.ctr").unwrap();
    writeln!(out, "  br label %{d}.loop").unwrap();
    writeln!(out, "{d}.loop:").unwrap();
    writeln!(out, "  %{d}.i = load i64, ptr %{d}.ctr").unwrap();
    writeln!(out, "  %{d}.done = icmp sge i64 %{d}.i, %{d}.len").unwrap();
    writeln!(out, "  br i1 %{d}.done, label %{d}.end, label %{d}.body").unwrap();
    writeln!(out, "{d}.body:").unwrap();
    writeln!(
        out,
        "  %{d}.srcp = getelementptr {elem_ty}, ptr %{d}.data, i64 %{d}.i"
    )
    .unwrap();
    writeln!(out, "  %{d}.elem = load {elem_ty}, ptr %{d}.srcp").unwrap();
    writeln!(
        out,
        "  %{d}.mapped = call {elem_ty} %{d}.fnp(ptr %{d}.envp, {elem_ty} %{d}.elem)"
    )
    .unwrap();
    writeln!(
        out,
        "  %{d}.dstp = getelementptr {elem_ty}, ptr %{d}.newdata, i64 %{d}.i"
    )
    .unwrap();
    writeln!(out, "  store {elem_ty} %{d}.mapped, ptr %{d}.dstp").unwrap();
    writeln!(out, "  %{d}.next = add i64 %{d}.i, 1").unwrap();
    writeln!(out, "  store i64 %{d}.next, ptr %{d}.ctr").unwrap();
    writeln!(out, "  br label %{d}.loop").unwrap();
    writeln!(out, "{d}.end:").unwrap();
    writeln!(
        out,
        "  %{d}.s0 = insertvalue {list_llvm_ty} undef, ptr %{d}.newdata, 0"
    )
    .unwrap();
    writeln!(
        out,
        "  %{d} = insertvalue {list_llvm_ty} %{d}.s0, i64 %{d}.len, 1"
    )
    .unwrap();
}

/// `__list_filter_int(xs, f)` / `__list_filter_str(xs, f)` — keep elements for
/// which the predicate returns true. Returns a new list whose length ≤ input length.
fn emit_list_filter(
    out: &mut String,
    dest: Option<&str>,
    args: &[Operand],
    func: &Function,
    ctx: &EmitCtx,
    elem_ty: &str,
    list_struct: &str,
) {
    let d = dest.unwrap_or("_lfilt");
    let list_llvm_ty = if list_struct == "List__Int" {
        list_int_llvm_ty(ctx)
    } else {
        list_str_llvm_ty(ctx)
    };
    let list_val = args
        .first()
        .map(|a| operand_ref(a, func))
        .unwrap_or_else(|| "undef".into());
    let fat_val = args
        .get(1)
        .map(|a| operand_ref(a, func))
        .unwrap_or_else(|| "null".into());

    writeln!(
        out,
        "  %{d}.data = extractvalue {list_llvm_ty} {list_val}, 0"
    )
    .unwrap();
    writeln!(
        out,
        "  %{d}.len = extractvalue {list_llvm_ty} {list_val}, 1"
    )
    .unwrap();
    writeln!(out, "  %{d}.size = mul i64 %{d}.len, 8").unwrap();
    writeln!(out, "  %{d}.outdata = call ptr @GC_malloc(i64 %{d}.size)").unwrap();
    writeln!(out, "  %{d}.null = icmp eq ptr %{d}.outdata, null").unwrap();
    writeln!(out, "  br i1 %{d}.null, label %{d}.oom, label %{d}.setup").unwrap();
    writeln!(out, "{d}.oom:").unwrap();
    writeln!(out, "  call void @abort()").unwrap();
    writeln!(out, "  unreachable").unwrap();
    writeln!(out, "{d}.setup:").unwrap();
    emit_fat_ptr_load(out, d, &fat_val);
    writeln!(out, "  %{d}.ctr = alloca i64").unwrap();
    writeln!(out, "  store i64 0, ptr %{d}.ctr").unwrap();
    writeln!(out, "  %{d}.outctr = alloca i64").unwrap();
    writeln!(out, "  store i64 0, ptr %{d}.outctr").unwrap();
    writeln!(out, "  br label %{d}.loop").unwrap();
    writeln!(out, "{d}.loop:").unwrap();
    writeln!(out, "  %{d}.i = load i64, ptr %{d}.ctr").unwrap();
    writeln!(out, "  %{d}.done = icmp sge i64 %{d}.i, %{d}.len").unwrap();
    writeln!(out, "  br i1 %{d}.done, label %{d}.end, label %{d}.body").unwrap();
    writeln!(out, "{d}.body:").unwrap();
    writeln!(
        out,
        "  %{d}.srcp = getelementptr {elem_ty}, ptr %{d}.data, i64 %{d}.i"
    )
    .unwrap();
    writeln!(out, "  %{d}.elem = load {elem_ty}, ptr %{d}.srcp").unwrap();
    writeln!(
        out,
        "  %{d}.raw = call i1 %{d}.fnp(ptr %{d}.envp, {elem_ty} %{d}.elem)"
    )
    .unwrap();
    writeln!(out, "  br i1 %{d}.raw, label %{d}.keep, label %{d}.skip").unwrap();
    writeln!(out, "{d}.keep:").unwrap();
    writeln!(out, "  %{d}.oi = load i64, ptr %{d}.outctr").unwrap();
    writeln!(
        out,
        "  %{d}.dstp = getelementptr {elem_ty}, ptr %{d}.outdata, i64 %{d}.oi"
    )
    .unwrap();
    writeln!(out, "  store {elem_ty} %{d}.elem, ptr %{d}.dstp").unwrap();
    writeln!(out, "  %{d}.oi1 = add i64 %{d}.oi, 1").unwrap();
    writeln!(out, "  store i64 %{d}.oi1, ptr %{d}.outctr").unwrap();
    writeln!(out, "  br label %{d}.skip").unwrap();
    writeln!(out, "{d}.skip:").unwrap();
    writeln!(out, "  %{d}.next = add i64 %{d}.i, 1").unwrap();
    writeln!(out, "  store i64 %{d}.next, ptr %{d}.ctr").unwrap();
    writeln!(out, "  br label %{d}.loop").unwrap();
    writeln!(out, "{d}.end:").unwrap();
    writeln!(out, "  %{d}.outlen = load i64, ptr %{d}.outctr").unwrap();
    writeln!(
        out,
        "  %{d}.s0 = insertvalue {list_llvm_ty} undef, ptr %{d}.outdata, 0"
    )
    .unwrap();
    writeln!(
        out,
        "  %{d} = insertvalue {list_llvm_ty} %{d}.s0, i64 %{d}.outlen, 1"
    )
    .unwrap();
}

/// `__list_fold_int(xs, init, f)` / `__list_fold_str(xs, init, f)` — left fold.
/// `elem_ty`: LLVM type of both the accumulator and each element.
fn emit_list_fold(
    out: &mut String,
    dest: Option<&str>,
    args: &[Operand],
    func: &Function,
    ctx: &EmitCtx,
    elem_ty: &str,
) {
    let d = dest.unwrap_or("_lfold");
    let list_llvm_ty = if elem_ty == "i64" {
        list_int_llvm_ty(ctx)
    } else {
        list_str_llvm_ty(ctx)
    };
    let list_val = args
        .first()
        .map(|a| operand_ref(a, func))
        .unwrap_or_else(|| "undef".into());
    let init_val = args
        .get(1)
        .map(|a| operand_ref(a, func))
        .unwrap_or_else(|| "0".into());
    let fat_val = args
        .get(2)
        .map(|a| operand_ref(a, func))
        .unwrap_or_else(|| "null".into());

    writeln!(
        out,
        "  %{d}.data = extractvalue {list_llvm_ty} {list_val}, 0"
    )
    .unwrap();
    writeln!(
        out,
        "  %{d}.len = extractvalue {list_llvm_ty} {list_val}, 1"
    )
    .unwrap();
    emit_fat_ptr_load(out, d, &fat_val);
    writeln!(out, "  %{d}.acc = alloca {elem_ty}").unwrap();
    writeln!(out, "  store {elem_ty} {init_val}, ptr %{d}.acc").unwrap();
    writeln!(out, "  %{d}.ctr = alloca i64").unwrap();
    writeln!(out, "  store i64 0, ptr %{d}.ctr").unwrap();
    writeln!(out, "  br label %{d}.loop").unwrap();
    writeln!(out, "{d}.loop:").unwrap();
    writeln!(out, "  %{d}.i = load i64, ptr %{d}.ctr").unwrap();
    writeln!(out, "  %{d}.done = icmp sge i64 %{d}.i, %{d}.len").unwrap();
    writeln!(out, "  br i1 %{d}.done, label %{d}.end, label %{d}.body").unwrap();
    writeln!(out, "{d}.body:").unwrap();
    writeln!(
        out,
        "  %{d}.srcp = getelementptr {elem_ty}, ptr %{d}.data, i64 %{d}.i"
    )
    .unwrap();
    writeln!(out, "  %{d}.elem = load {elem_ty}, ptr %{d}.srcp").unwrap();
    writeln!(out, "  %{d}.cur = load {elem_ty}, ptr %{d}.acc").unwrap();
    writeln!(
        out,
        "  %{d}.new = call {elem_ty} %{d}.fnp(ptr %{d}.envp, {elem_ty} %{d}.cur, {elem_ty} %{d}.elem)"
    )
    .unwrap();
    writeln!(out, "  store {elem_ty} %{d}.new, ptr %{d}.acc").unwrap();
    writeln!(out, "  %{d}.next = add i64 %{d}.i, 1").unwrap();
    writeln!(out, "  store i64 %{d}.next, ptr %{d}.ctr").unwrap();
    writeln!(out, "  br label %{d}.loop").unwrap();
    writeln!(out, "{d}.end:").unwrap();
    writeln!(out, "  %{d} = load {elem_ty}, ptr %{d}.acc").unwrap();
}

/// `__log_info/warn/error(msg) -> Unit` — emit a call to a `tyra_log_*` C function.
fn emit_log_call(out: &mut String, fn_name: &str, args: &[Operand], func: &Function) {
    let msg = args
        .first()
        .map(|a| operand_ref(a, func))
        .unwrap_or_else(|| "null".into());
    writeln!(out, "  call void @{fn_name}(ptr {msg})").unwrap();
}
