//! Inkwell I4a: table-driven "mechanical" builtin calls.
//!
//! The legacy text backend (`builtins.rs`) spells out ~55 builtins as one
//! hand-written `emit_*` helper each, because the string backend has no value
//! types and must annotate every `call`/argument explicitly. In the inkwell
//! value-handle model a builtin call is just `build_call` to the runtime extern
//! declared in I1 — the callee signature drives argument/return typing and the
//! operand handles already carry their LLVM types. So the whole mechanical
//! subset collapses to one data table of `(MIR name, runtime callee, result
//! conversion)`, mirroring I1's data-driven `declare_externs`.
//!
//! Scope (I4a): fs / json / http-client / io / string (scalar) / time / log /
//! float / bench — every builtin whose legacy helper is a single runtime call
//! plus at most a trivial result conversion. Deferred to later I4 sub-phases:
//! print/panic/sys (formatting + sentinels), the http *server* handle round
//! trips (ptr↔int), string split/join/replace (build List results), Map/Set/
//! LinkedMap/LinkedSet (dynamic names + eq/hash fn addresses), list map/filter/
//! fold (closure callbacks), and Int/Bool conversions.

use inkwell::IntPredicate;
use inkwell::values::{BasicMetadataValueEnum, CallSiteValue, PointerValue};

use tyra_mir::{Constant, Operand};

use crate::inkwell_codegen::CodeGen;

/// Printable scalar kind for a `print` argument (selects the printf format).
#[derive(Clone, Copy)]
enum PrintKind {
    Str,
    Float,
    Bool,
    Int,
}

/// How to adapt a runtime call's result to the Tyra value the MIR expects.
#[derive(Clone, Copy)]
enum Conv {
    /// Store the call result unchanged (matching LLVM types).
    Direct,
    /// Runtime returns `i32` (0/1); compare `!= 0` to a Tyra `Bool` (i1). The
    /// legacy backend uses `trunc i32→i1` for a few of these and `icmp ne 0`
    /// for the rest; both agree on the runtime's 0/1 contract, so this unifies
    /// on the robust form.
    Bool,
    /// Runtime returns `i32` errno; sign-extend to the Tyra `Int` (i64).
    Sext,
}

use Conv::*;

/// `(MIR builtin name, runtime extern, result conversion)`. Every callee is
/// declared in I1 (`declare_externs`). Void-returning entries (fs_write, log_*)
/// carry no dest and store nothing — handled uniformly by the absence of a
/// basic return value.
#[rustfmt::skip]
const SIMPLE: &[(&str, &str, Conv)] = &[
    // fs
    ("__fs_read_raw",   "tyra_fs_read",   Direct),
    ("__fs_errno",      "tyra_fs_errno",  Sext),
    ("__fs_errmsg",     "tyra_fs_errmsg", Direct),
    ("__fs_write_raw",  "tyra_fs_write",  Direct), // void
    ("__fs_exists",     "tyra_fs_exists", Bool),
    // json
    ("__json_parse",     "tyra_json_parse",     Direct),
    ("__json_err_msg",   "tyra_json_err_msg",   Direct),
    ("__json_err_line",  "tyra_json_err_line",  Direct),
    ("__json_err_col",   "tyra_json_err_col",   Direct),
    ("__json_kind",      "tyra_json_kind",      Direct),
    ("__json_is_string", "tyra_json_is_string", Bool),
    ("__json_is_int",    "tyra_json_is_int",    Bool),
    ("__json_is_bool",   "tyra_json_is_bool",   Bool),
    ("__json_str",       "tyra_json_str",       Direct),
    ("__json_int",       "tyra_json_int",       Direct),
    ("__json_bool",      "tyra_json_bool",      Bool),
    ("__json_get",       "tyra_json_get",       Direct),
    ("__json_at",        "tyra_json_at",        Direct),
    // http client (server handle round-trips deferred to I4b)
    ("__http_get",    "tyra_http_get",    Direct),
    ("__http_status", "tyra_http_status", Direct),
    ("__http_body",   "tyra_http_body",   Direct),
    ("__http_errno",  "tyra_http_errno",  Sext),
    ("__http_errmsg", "tyra_http_errmsg", Direct),
    // io
    ("__io_read_line",   "tyra_io_read_line",   Direct),
    ("__io_read_to_end", "tyra_io_read_to_end", Direct),
    ("__io_eof",         "tyra_io_eof",         Bool),
    // string (scalar; split/join/replace build Lists → deferred)
    ("__string_len",         "tyra_string_len",         Direct),
    ("__string_is_empty",    "tyra_string_is_empty",    Bool),
    ("__string_trim",        "tyra_string_trim",        Direct),
    ("__string_to_upper",    "tyra_string_to_upper",    Direct),
    ("__string_to_lower",    "tyra_string_to_lower",    Direct),
    ("__string_contains",    "tyra_string_contains",    Bool),
    ("__string_starts_with", "tyra_string_starts_with", Bool),
    ("__string_ends_with",   "tyra_string_ends_with",   Bool),
    ("__string_parse_int",   "tyra_string_parse_int",   Direct),
    ("__string_parse_errno", "tyra_string_parse_errno", Sext),
    ("__string_byte_at",     "tyra_string_byte_at",     Direct),
    ("__string_substring",   "tyra_string_substring",   Direct),
    ("__string_reverse",     "tyra_string_reverse",     Direct),
    ("__string_from_byte",   "tyra_string_from_byte",   Direct),
    // time
    ("__time_now_unix",         "tyra_time_now_unix",         Direct),
    ("__time_monotonic_millis", "tyra_time_monotonic_millis", Direct),
    // log (void)
    ("__log_info",  "tyra_log_info",  Direct),
    ("__log_warn",  "tyra_log_warn",  Direct),
    ("__log_error", "tyra_log_error", Direct),
    // float
    ("__float_eq",          "tyra_float_eq",          Bool),
    ("__float_approx_eq",   "tyra_float_approx_eq",   Bool),
    ("__float_abs",         "tyra_float_abs",         Direct),
    ("__float_floor",       "tyra_float_floor",       Direct),
    ("__float_ceil",        "tyra_float_ceil",        Direct),
    ("__float_round",       "tyra_float_round",       Direct),
    ("__float_min",         "tyra_float_min",         Direct),
    ("__float_max",         "tyra_float_max",         Direct),
    ("__float_to_string",   "tyra_float_to_string",   Direct),
    ("__float_parse",       "tyra_float_parse",       Direct),
    ("__float_parse_errno", "tyra_float_parse_errno", Sext),
    ("__float_from_int",    "tyra_float_from_int",    Direct),
    ("__float_to_int",      "tyra_float_to_int",      Direct),
    ("__float_is_nan",      "tyra_float_is_nan",      Bool),
    ("__float_is_infinite", "tyra_float_is_infinite", Bool),
    // bench
    ("__bench_clock_ns", "__bench_clock_ns", Direct),
];

/// I4c: the print family. Routed separately from the table — the format and
/// call shape depend on the argument's *Tyra* type (via the type scan), which
/// the LLVM value handle alone can't recover (String vs other ptr).
const PRINT: &[&str] = &["print", "eprint", "println", "eprintln"];

impl<'ctx> CodeGen<'ctx> {
    /// Is `name` a print-family builtin?
    pub(crate) fn is_print_builtin(name: &str) -> bool {
        PRINT.contains(&name)
    }

    /// Is `name` a builtin the inkwell backend can emit yet? Used by the
    /// emittability gate so a function calling only supported builtins (and
    /// user fns) gets a real body instead of the `unreachable` fallback.
    pub(crate) fn is_supported_builtin(name: &str) -> bool {
        PRINT.contains(&name) || SIMPLE.iter().any(|(f, _, _)| *f == name)
    }

    /// Emit a builtin call. Returns `false` if `fname` is not supported (caller
    /// falls through to the fallback path).
    pub(crate) fn emit_builtin(
        &mut self,
        dest: &Option<String>,
        fname: &str,
        args: &[Operand],
    ) -> bool {
        if PRINT.contains(&fname) {
            self.emit_print(dest, fname, args);
            return true;
        }
        self.emit_simple_builtin(dest, fname, args)
    }

    /// `print`/`println`/`eprint`/`eprintln`. The argument's *Tyra* type (from
    /// the type scan) selects the format, mirroring the legacy `emit_print_call`
    /// EXACTLY (the migration's correctness bar is parity, not "fix print"):
    /// - `string_temps` → `%s` (`puts` when newline-terminated). NOTE the scan
    ///   puts **data types in `string_temps`** too (type_scan.rs:154/182, "data
    ///   ptr treated as ptr"), so `print(dataObject)` routes to `%s` and the
    ///   runtime reads its bytes as a C string — a latent legacy behavior,
    ///   faithfully preserved here (revisit only post-I7, when both backends are
    ///   one). Struct *value* args (List/Option/closure) are a different case:
    ///   rejected upstream by the gate so they never reach here.
    /// - Float → `%g`(`_ln`); Bool → widened-i64 `%ld`(`_ln`).
    /// - else → `%ld` (Int). An untracked non-String ptr (e.g. a fn pointer) is
    ///   *not* in `string_temps`, so it lands here and prints as its address via
    ///   the varargs `%ld` — never dereferenced (matches legacy intent; legacy
    ///   itself would have emitted invalid `i64 %ptr` IR, but this backend's
    ///   varargs accept the ptr and print the address).
    /// printf/puts return i32; a dest (byte count, Int) is sign-extended to i64.
    fn emit_print(&mut self, dest: &Option<String>, fname: &str, args: &[Operand]) {
        let is_println = fname == "println" || fname == "eprintln";

        // Empty args: `println()` prints a blank line (legacy `puts(@.fmt.str)`);
        // `print()` is a no-op.
        if args.is_empty() {
            if is_println {
                let puts = self.module.get_function("puts").unwrap();
                let fmt = self.global_ptr(".fmt.str");
                let cs = self.builder.build_call(puts, &[fmt.into()], "").unwrap();
                self.store_print_result(dest, cs);
            }
            return;
        }

        let arg = &args[0];
        let v = self.operand(arg);
        let cs = match self.print_arg_kind(arg) {
            PrintKind::Str if is_println => {
                // String + newline: puts(s).
                let puts = self.module.get_function("puts").unwrap();
                self.builder.build_call(puts, &[v.into()], "").unwrap()
            }
            kind => {
                let (fmt_name, val): (&str, BasicMetadataValueEnum<'ctx>) = match kind {
                    PrintKind::Str => (".fmt.str", v.into()),
                    PrintKind::Float => {
                        (if is_println { ".fmt.float_ln" } else { ".fmt.float" }, v.into())
                    }
                    PrintKind::Bool => {
                        let wide = self
                            .builder
                            .build_int_z_extend(v.into_int_value(), self.ctx.i64_type(), "p.wide")
                            .unwrap();
                        (if is_println { ".fmt.int_ln" } else { ".fmt.int" }, wide.into())
                    }
                    PrintKind::Int => {
                        (if is_println { ".fmt.int_ln" } else { ".fmt.int" }, v.into())
                    }
                };
                let printf = self.module.get_function("printf").unwrap();
                let fmt = self.global_ptr(fmt_name);
                self.builder.build_call(printf, &[fmt.into(), val], "").unwrap()
            }
        };
        self.store_print_result(dest, cs);
    }

    /// Classify a print argument by its Tyra type, mirroring the legacy
    /// `string_temps`/`float_temps`/`bool_temps` scan (data types live in
    /// `string_temps`, so they route to `%s` exactly as the legacy does).
    fn print_arg_kind(&self, op: &Operand) -> PrintKind {
        match op {
            Operand::Const(Constant::StringRef(_)) => PrintKind::Str,
            Operand::Const(Constant::Float(_)) => PrintKind::Float,
            Operand::Const(Constant::Bool(_)) => PrintKind::Bool,
            Operand::Const(_) => PrintKind::Int,
            Operand::Var(name) => {
                let scan = self.scan.as_ref().expect("type scan set per function (I4c)");
                if scan.string_temps.contains(name) {
                    PrintKind::Str
                } else if scan.float_temps.contains(name) {
                    PrintKind::Float
                } else if scan.bool_temps.contains(name) {
                    PrintKind::Bool
                } else {
                    PrintKind::Int
                }
            }
        }
    }

    /// A `ptr` to a module global by name (format string / constant).
    fn global_ptr(&self, name: &str) -> PointerValue<'ctx> {
        self.module
            .get_global(name)
            .unwrap_or_else(|| panic!("global `{name}` must be declared (I1)"))
            .as_pointer_value()
    }

    /// printf/puts return i32; store the sign-extended i64 byte count if the
    /// call carries a dest.
    fn store_print_result(&mut self, dest: &Option<String>, cs: CallSiteValue<'ctx>) {
        let Some(d) = dest else { return };
        let Some(rv) = cs.try_as_basic_value().basic() else { return };
        let v = self
            .builder
            .build_int_s_extend(rv.into_int_value(), self.ctx.i64_type(), d)
            .unwrap();
        self.values.insert(d.clone(), v.into());
    }

    /// Emit a table-driven builtin call. Returns `false` if `fname` is not in
    /// the I4a table (caller falls through to the fallback path).
    fn emit_simple_builtin(
        &mut self,
        dest: &Option<String>,
        fname: &str,
        args: &[Operand],
    ) -> bool {
        let Some((_, callee, conv)) = SIMPLE.iter().find(|(f, _, _)| *f == fname) else {
            return false;
        };
        let f = self
            .module
            .get_function(callee)
            .unwrap_or_else(|| panic!("runtime extern `{callee}` must be declared (I1)"));
        let argvals: Vec<BasicMetadataValueEnum<'ctx>> =
            args.iter().map(|a| self.operand(a).into()).collect();

        // For a converted result the raw call is the `.i32` intermediate; for a
        // direct result it *is* the dest value.
        let raw_name = match (dest, conv) {
            (Some(d), Conv::Direct) => d.clone(),
            (Some(d), _) => format!("{d}.i32"),
            (None, _) => String::new(),
        };
        let cs = self.builder.build_call(f, &argvals, &raw_name).unwrap();

        let Some(d) = dest else { return true };
        let Some(rv) = cs.try_as_basic_value().basic() else {
            return true; // void runtime fn (shouldn't carry a dest, but be safe)
        };
        let v = match conv {
            Conv::Direct => rv,
            Conv::Bool => {
                let i = rv.into_int_value();
                let zero = self.ctx.i32_type().const_zero();
                self.builder
                    .build_int_compare(IntPredicate::NE, i, zero, d)
                    .unwrap()
                    .into()
            }
            Conv::Sext => self
                .builder
                .build_int_s_extend(rv.into_int_value(), self.ctx.i64_type(), d)
                .unwrap()
                .into(),
        };
        self.values.insert(d.clone(), v);
        true
    }
}
