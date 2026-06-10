//! Float stdlib backing (§17.3.x). v0.1 scalar float operations.
//!
//! Exposes `tyra_float_*` intrinsics consumed by `stdlib/float.ty`.
//!
//! Scope (v0.1): equality, approximate equality, abs, floor, ceil, round,
//! to_string, parse (with errno), from_int, to_int (truncate), min, max.
//!
//! All returned strings are allocated via `gc_string::alloc_gc_cstring`,
//! which uses `GC_malloc_atomic` so the Boehm GC manages their lifetime.
//!
//! Bool returns use `c_int` (1=true, 0=false); codegen truncates `i32 → i1`.
//! Int returns use `i64`, Float returns use `f64`, String returns use `*const c_char`.

use crate::gc_string::alloc_gc_cstring;
use std::cell::Cell;
use std::os::raw::{c_char, c_int};

thread_local! {
    /// parse_float result flag: 0 = Ok, 1 = ParseFailed.
    /// Meaningful only immediately after `tyra_float_parse`.
    static FLOAT_PARSE_ERRNO: Cell<c_int> = const { Cell::new(0) };
}

/// `__float_eq(a, b) -> Bool` — exact float equality.
#[unsafe(no_mangle)]
pub extern "C" fn tyra_float_eq(a: f64, b: f64) -> c_int {
    if a == b { 1 } else { 0 }
}

/// `__float_approx_eq(a, b, eps) -> Bool` — |a - b| <= eps.
#[unsafe(no_mangle)]
pub extern "C" fn tyra_float_approx_eq(a: f64, b: f64, eps: f64) -> c_int {
    if (a - b).abs() <= eps { 1 } else { 0 }
}

/// `__float_abs(x) -> Float` — absolute value.
#[unsafe(no_mangle)]
pub extern "C" fn tyra_float_abs(x: f64) -> f64 {
    x.abs()
}

/// `__float_floor(x) -> Float` — floor.
#[unsafe(no_mangle)]
pub extern "C" fn tyra_float_floor(x: f64) -> f64 {
    x.floor()
}

/// `__float_ceil(x) -> Float` — ceiling.
#[unsafe(no_mangle)]
pub extern "C" fn tyra_float_ceil(x: f64) -> f64 {
    x.ceil()
}

/// `__float_round(x) -> Float` — round to nearest, ties away from zero
/// (Rust's `f64::round`).
#[unsafe(no_mangle)]
pub extern "C" fn tyra_float_round(x: f64) -> f64 {
    x.round()
}

/// `__float_min(a, b) -> Float` — minimum of two floats.
#[unsafe(no_mangle)]
pub extern "C" fn tyra_float_min(a: f64, b: f64) -> f64 {
    a.min(b)
}

/// `__float_max(a, b) -> Float` — maximum of two floats.
#[unsafe(no_mangle)]
pub extern "C" fn tyra_float_max(a: f64, b: f64) -> f64 {
    a.max(b)
}

/// `__float_to_string(x) -> String` — decimal representation.
///
/// Integer-valued floats always include a decimal point (e.g. `0.0`, `1.0`)
/// so that the output is unambiguously a Float, not an Int.
/// Non-finite values use Rust's standard forms: `inf`, `-inf`, `NaN`.
#[unsafe(no_mangle)]
pub extern "C" fn tyra_float_to_string(x: f64) -> *const c_char {
    let s = if x.is_finite() && x.fract() == 0.0 {
        format!("{:.1}", x)
    } else {
        format!("{}", x)
    };
    alloc_gc_cstring(&s)
}

/// `__float_parse(s) -> Float` — parse a float from a string.
///
/// Sets the thread-local errno to 0 on success, 1 on failure.
/// Returns 0.0 on failure. Caller should check `__float_parse_errno`.
///
/// # Safety
/// `s` must be a null-terminated UTF-8 string (or null).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tyra_float_parse(s: *const c_char) -> f64 {
    let input = if s.is_null() {
        ""
    } else {
        unsafe { std::ffi::CStr::from_ptr(s) }
            .to_str()
            .unwrap_or("")
    };
    match input.parse::<f64>() {
        Ok(v) => {
            FLOAT_PARSE_ERRNO.with(|e| e.set(0));
            v
        }
        Err(_) => {
            FLOAT_PARSE_ERRNO.with(|e| e.set(1));
            0.0
        }
    }
}

/// Return 0 on success / 1 on parse failure for the most recent
/// `tyra_float_parse` call on the calling thread.
#[unsafe(no_mangle)]
pub extern "C" fn tyra_float_parse_errno() -> c_int {
    FLOAT_PARSE_ERRNO.with(|e| e.get())
}

/// `__float_from_int(n) -> Float` — convert Int to Float (i64 → f64).
#[unsafe(no_mangle)]
pub extern "C" fn tyra_float_from_int(n: i64) -> f64 {
    n as f64
}

/// `__float_to_int(x) -> Int` — truncate Float to Int (f64 → i64, C truncation semantics).
#[unsafe(no_mangle)]
pub extern "C" fn tyra_float_to_int(x: f64) -> i64 {
    x as i64
}

/// `__float_is_nan(x) -> Bool` — true iff x is NaN.
#[unsafe(no_mangle)]
pub extern "C" fn tyra_float_is_nan(x: f64) -> c_int {
    if x.is_nan() { 1 } else { 0 }
}

/// `__float_is_infinite(x) -> Bool` — true iff x is +∞ or −∞.
#[unsafe(no_mangle)]
pub extern "C" fn tyra_float_is_infinite(x: f64) -> c_int {
    if x.is_infinite() { 1 } else { 0 }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::CString;

    fn cs(s: &str) -> CString {
        CString::new(s).unwrap()
    }

    #[test]
    fn eq_exact() {
        assert_eq!(tyra_float_eq(1.0, 1.0), 1);
        assert_eq!(tyra_float_eq(1.0, 1.1), 0);
        assert_eq!(tyra_float_eq(0.0, -0.0), 1); // IEEE 754: 0.0 == -0.0
    }

    #[test]
    fn approx_eq_within_eps() {
        // 0.1 + 0.2 differs from 0.3 by ~5.5e-17; inside 1e-10 and 1e-9.
        assert_eq!(tyra_float_approx_eq(0.1 + 0.2, 0.3, 1e-18), 0); // 5.5e-17 > 1e-18
        assert_eq!(tyra_float_approx_eq(0.1 + 0.2, 0.3, 1e-10), 1); // 5.5e-17 < 1e-10
        assert_eq!(tyra_float_approx_eq(1.0, 2.0, 0.5), 0);
        assert_eq!(tyra_float_approx_eq(1.0, 1.5, 0.5), 1);
    }

    #[test]
    fn abs_values() {
        assert_eq!(tyra_float_abs(-3.5), 3.5);
        assert_eq!(tyra_float_abs(3.5), 3.5);
        assert_eq!(tyra_float_abs(0.0), 0.0);
    }

    #[test]
    fn floor_ceil_round() {
        assert_eq!(tyra_float_floor(3.7), 3.0);
        assert_eq!(tyra_float_floor(-3.7), -4.0);
        assert_eq!(tyra_float_ceil(3.2), 4.0);
        assert_eq!(tyra_float_ceil(-3.2), -3.0);
        assert_eq!(tyra_float_round(3.5), 4.0);
        assert_eq!(tyra_float_round(3.4), 3.0);
        assert_eq!(tyra_float_round(-3.5), -4.0);
    }

    #[test]
    fn min_max_values() {
        assert_eq!(tyra_float_min(1.0, 2.0), 1.0);
        assert_eq!(tyra_float_min(-1.0, 0.0), -1.0);
        assert_eq!(tyra_float_max(1.0, 2.0), 2.0);
        assert_eq!(tyra_float_max(-1.0, 0.0), 0.0);
    }

    #[test]
    fn to_string_decimal() {
        let p = tyra_float_to_string(3.14);
        let s = unsafe { std::ffi::CStr::from_ptr(p) }.to_str().unwrap();
        assert_eq!(s, "3.14");
        let p = tyra_float_to_string(0.0);
        let s = unsafe { std::ffi::CStr::from_ptr(p) }.to_str().unwrap();
        assert_eq!(s, "0.0");
        let p = tyra_float_to_string(-1.5);
        let s = unsafe { std::ffi::CStr::from_ptr(p) }.to_str().unwrap();
        assert_eq!(s, "-1.5");
    }

    #[test]
    fn to_string_integer_valued_preserves_dot_zero() {
        for (input, expected) in [
            (1.0_f64, "1.0"),
            (-2.0_f64, "-2.0"),
            (100.0_f64, "100.0"),
            (f64::INFINITY, "inf"),
            (f64::NEG_INFINITY, "-inf"),
            (f64::NAN, "NaN"),
        ] {
            let p = tyra_float_to_string(input);
            let s = unsafe { std::ffi::CStr::from_ptr(p) }.to_str().unwrap();
            assert_eq!(s, expected, "tyra_float_to_string({input}) wrong");
        }
    }

    #[test]
    fn parse_errno_roundtrip() {
        let v = unsafe { tyra_float_parse(cs("3.14").as_ptr()) };
        assert!((v - 3.14).abs() < 1e-10);
        assert_eq!(tyra_float_parse_errno(), 0);

        let v = unsafe { tyra_float_parse(cs("not-a-float").as_ptr()) };
        assert_eq!(v, 0.0);
        assert_eq!(tyra_float_parse_errno(), 1);

        // Null pointer → parse failure
        let v = unsafe { tyra_float_parse(std::ptr::null()) };
        assert_eq!(v, 0.0);
        assert_eq!(tyra_float_parse_errno(), 1);
    }

    #[test]
    fn from_int_to_int_roundtrip() {
        assert_eq!(tyra_float_from_int(42), 42.0);
        assert_eq!(tyra_float_from_int(-7), -7.0);
        assert_eq!(tyra_float_to_int(3.9), 3);
        assert_eq!(tyra_float_to_int(-3.9), -3);
        assert_eq!(tyra_float_to_int(0.0), 0);
    }

    #[test]
    fn is_nan_values() {
        assert_eq!(tyra_float_is_nan(f64::NAN), 1);
        assert_eq!(tyra_float_is_nan(0.0), 0);
        assert_eq!(tyra_float_is_nan(1.0), 0);
        assert_eq!(tyra_float_is_nan(f64::INFINITY), 0);
        assert_eq!(tyra_float_is_nan(f64::NEG_INFINITY), 0);
    }

    #[test]
    fn is_infinite_values() {
        assert_eq!(tyra_float_is_infinite(f64::INFINITY), 1);
        assert_eq!(tyra_float_is_infinite(f64::NEG_INFINITY), 1);
        assert_eq!(tyra_float_is_infinite(0.0), 0);
        assert_eq!(tyra_float_is_infinite(1.0), 0);
        assert_eq!(tyra_float_is_infinite(f64::NAN), 0);
    }
}
