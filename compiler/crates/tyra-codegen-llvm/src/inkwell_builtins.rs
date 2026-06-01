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
use inkwell::values::BasicMetadataValueEnum;

use tyra_mir::Operand;

use crate::inkwell_codegen::CodeGen;

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

impl<'ctx> CodeGen<'ctx> {
    /// Is `name` a builtin handled by the I4a table? Used by the emittability
    /// gate so a function calling only supported builtins (and user fns) gets a
    /// real body instead of the `unreachable` fallback.
    pub(crate) fn is_simple_builtin(name: &str) -> bool {
        SIMPLE.iter().any(|(f, _, _)| *f == name)
    }

    /// Emit a table-driven builtin call. Returns `false` if `fname` is not in
    /// the I4a table (caller falls through to the fallback path).
    pub(crate) fn emit_simple_builtin(
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
