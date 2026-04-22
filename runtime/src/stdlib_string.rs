//! String stdlib backing (§17.3.4). v0.1 scalar string operations.
//!
//! Exposes `tyra_string_*` intrinsics consumed by `stdlib/string.tyra`.
//!
//! Scope (v0.1): byte-length, trim, upper/lower (ASCII only), substring
//! predicates (contains / starts_with / ends_with), and decimal parse_int
//! with a thread-local errno. Splitting into `List<String>` is intentionally
//! deferred — List construction requires compiler-owned struct layout, and
//! the plumbing lands in a later phase together with a generalized
//! "intrinsic returns List<String>" codegen helper.
//!
//! All returned strings are allocated via `CString::into_raw` (same
//! trade-off as fs/io: Boehm GC scans conservatively, buffers never
//! freed in v0.1). Input strings are borrowed from the caller's C
//! string; we never mutate them.

use std::cell::Cell;
use std::ffi::{CStr, CString};
use std::os::raw::{c_char, c_int};

thread_local! {
    /// parse_int result flag: 0 = Ok, 1 = ParseFailed.
    /// Meaningful only immediately after `tyra_string_parse_int`.
    static STRING_PARSE_ERRNO: Cell<c_int> = const { Cell::new(0) };
}

fn leak_cstring(s: String) -> *const c_char {
    let mut cleaned = s;
    if let Some(pos) = cleaned.as_bytes().iter().position(|&b| b == 0) {
        cleaned.truncate(pos);
    }
    match CString::new(cleaned) {
        Ok(c) => c.into_raw(),
        Err(_) => CString::new("").unwrap().into_raw(),
    }
}

/// Borrow a `&str` from a caller-provided C string. Returns `""` when the
/// pointer is null or the contents are not valid UTF-8.
fn borrow_utf8<'a>(ptr: *const c_char) -> &'a str {
    if ptr.is_null() {
        return "";
    }
    unsafe { CStr::from_ptr(ptr) }.to_str().unwrap_or("")
}

/// `__string_len(s) -> Int` — byte length (UTF-8 bytes).
///
/// # Safety
/// `s` must be a null-terminated UTF-8 string (or null).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tyra_string_len(s: *const c_char) -> i64 {
    borrow_utf8(s).len() as i64
}

/// `__string_is_empty(s) -> Bool` — true iff `s` has zero bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tyra_string_is_empty(s: *const c_char) -> c_int {
    if borrow_utf8(s).is_empty() { 1 } else { 0 }
}

/// `__string_trim(s) -> String` — strip leading/trailing ASCII whitespace.
/// Non-ASCII whitespace (e.g. U+3000) is NOT trimmed in v0.1.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tyra_string_trim(s: *const c_char) -> *const c_char {
    let input = borrow_utf8(s);
    // Trim ASCII whitespace only — UTF-8 is byte-safe because ASCII bytes
    // cannot appear in multi-byte sequences.
    let bytes = input.as_bytes();
    let out = match bytes.iter().position(|b| !b.is_ascii_whitespace()) {
        None => "", // all whitespace (or empty)
        Some(start) => {
            // `start` found a non-whitespace byte, so `rposition` is guaranteed Some.
            let end = bytes
                .iter()
                .rposition(|b| !b.is_ascii_whitespace())
                .map(|i| i + 1)
                .unwrap_or(start);
            &input[start..end]
        }
    };
    leak_cstring(out.to_string())
}

/// `__string_to_upper(s) -> String` — ASCII-only upper-casing.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tyra_string_to_upper(s: *const c_char) -> *const c_char {
    let out: String = borrow_utf8(s)
        .chars()
        .map(|c| if c.is_ascii() { c.to_ascii_uppercase() } else { c })
        .collect();
    leak_cstring(out)
}

/// `__string_to_lower(s) -> String` — ASCII-only lower-casing.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tyra_string_to_lower(s: *const c_char) -> *const c_char {
    let out: String = borrow_utf8(s)
        .chars()
        .map(|c| if c.is_ascii() { c.to_ascii_lowercase() } else { c })
        .collect();
    leak_cstring(out)
}

/// `__string_contains(s, needle) -> Bool`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tyra_string_contains(s: *const c_char, needle: *const c_char) -> c_int {
    if borrow_utf8(s).contains(borrow_utf8(needle)) { 1 } else { 0 }
}

/// `__string_starts_with(s, prefix) -> Bool`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tyra_string_starts_with(s: *const c_char, prefix: *const c_char) -> c_int {
    if borrow_utf8(s).starts_with(borrow_utf8(prefix)) { 1 } else { 0 }
}

/// `__string_ends_with(s, suffix) -> Bool`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tyra_string_ends_with(s: *const c_char, suffix: *const c_char) -> c_int {
    if borrow_utf8(s).ends_with(borrow_utf8(suffix)) { 1 } else { 0 }
}

/// `__string_parse_int(s) -> Int` — decimal parse, with a thread-local
/// success flag read via `__string_parse_errno`. Returns 0 on failure
/// (and sets errno=1).
///
/// Accepts optional leading '-' / '+' and ASCII decimal digits. Leading
/// / trailing whitespace is rejected — callers should trim first.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tyra_string_parse_int(s: *const c_char) -> i64 {
    let input = borrow_utf8(s);
    match input.parse::<i64>() {
        Ok(n) => {
            STRING_PARSE_ERRNO.with(|e| e.set(0));
            n
        }
        Err(_) => {
            STRING_PARSE_ERRNO.with(|e| e.set(1));
            0
        }
    }
}

/// Return 0 on success / 1 on parse failure for the most recent
/// `tyra_string_parse_int` call on the calling thread.
#[unsafe(no_mangle)]
pub extern "C" fn tyra_string_parse_errno() -> c_int {
    STRING_PARSE_ERRNO.with(|e| e.get())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cs(s: &str) -> CString {
        CString::new(s).unwrap()
    }

    #[test]
    fn len_counts_utf8_bytes() {
        let ascii = cs("hello");
        assert_eq!(unsafe { tyra_string_len(ascii.as_ptr()) }, 5);
        // "あ" is 3 UTF-8 bytes.
        let ja = cs("あ");
        assert_eq!(unsafe { tyra_string_len(ja.as_ptr()) }, 3);
        let empty = cs("");
        assert_eq!(unsafe { tyra_string_len(empty.as_ptr()) }, 0);
    }

    #[test]
    fn is_empty_matches_len_zero() {
        assert_eq!(unsafe { tyra_string_is_empty(cs("").as_ptr()) }, 1);
        assert_eq!(unsafe { tyra_string_is_empty(cs("x").as_ptr()) }, 0);
    }

    #[test]
    fn trim_strips_ascii_whitespace_only() {
        let p = unsafe { tyra_string_trim(cs("  hi \n").as_ptr()) };
        let got = unsafe { CStr::from_ptr(p) }.to_str().unwrap();
        assert_eq!(got, "hi");
        // All whitespace → empty.
        let p = unsafe { tyra_string_trim(cs("   ").as_ptr()) };
        assert_eq!(unsafe { CStr::from_ptr(p) }.to_str().unwrap(), "");
    }

    #[test]
    fn upper_lower_ascii() {
        let up = unsafe { tyra_string_to_upper(cs("Hello, あ").as_ptr()) };
        assert_eq!(unsafe { CStr::from_ptr(up) }.to_str().unwrap(), "HELLO, あ");
        let lo = unsafe { tyra_string_to_lower(cs("Hello, あ").as_ptr()) };
        assert_eq!(unsafe { CStr::from_ptr(lo) }.to_str().unwrap(), "hello, あ");
    }

    #[test]
    fn substring_predicates() {
        let s = cs("hello world");
        let ok = cs("world");
        let no = cs("WORLD");
        assert_eq!(unsafe { tyra_string_contains(s.as_ptr(), ok.as_ptr()) }, 1);
        assert_eq!(unsafe { tyra_string_contains(s.as_ptr(), no.as_ptr()) }, 0);
        let hello = cs("hello");
        assert_eq!(unsafe { tyra_string_starts_with(s.as_ptr(), hello.as_ptr()) }, 1);
        let worl = cs("worl");
        assert_eq!(unsafe { tyra_string_starts_with(s.as_ptr(), worl.as_ptr()) }, 0);
        assert_eq!(unsafe { tyra_string_ends_with(s.as_ptr(), ok.as_ptr()) }, 1);
    }

    #[test]
    fn parse_int_errno_roundtrip() {
        let good = cs("-42");
        let n = unsafe { tyra_string_parse_int(good.as_ptr()) };
        assert_eq!(n, -42);
        assert_eq!(tyra_string_parse_errno(), 0);
        let bad = cs("not-a-number");
        let n = unsafe { tyra_string_parse_int(bad.as_ptr()) };
        assert_eq!(n, 0);
        assert_eq!(tyra_string_parse_errno(), 1);
        // Trailing whitespace is rejected — parse_int is strict.
        let trailing = cs("42 ");
        let _ = unsafe { tyra_string_parse_int(trailing.as_ptr()) };
        assert_eq!(tyra_string_parse_errno(), 1);
    }

    #[test]
    fn null_pointer_is_empty_string() {
        assert_eq!(unsafe { tyra_string_len(std::ptr::null()) }, 0);
        assert_eq!(unsafe { tyra_string_is_empty(std::ptr::null()) }, 1);
    }
}
