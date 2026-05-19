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
//! All returned strings are allocated via `gc_string::alloc_gc_cstring`,
//! which uses `GC_malloc_atomic` so the Boehm GC manages their lifetime.
//! Input strings are borrowed from the caller's C string; we never mutate them.

use crate::gc_string::alloc_gc_cstring;
use std::cell::Cell;
use std::ffi::CStr;
use std::os::raw::{c_char, c_int};

thread_local! {
    /// parse_int result flag: 0 = Ok, 1 = ParseFailed.
    /// Meaningful only immediately after `tyra_string_parse_int`.
    static STRING_PARSE_ERRNO: Cell<c_int> = const { Cell::new(0) };
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
    alloc_gc_cstring(out)
}

/// `__string_to_upper(s) -> String` — ASCII-only upper-casing.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tyra_string_to_upper(s: *const c_char) -> *const c_char {
    let out: String = borrow_utf8(s)
        .chars()
        .map(|c| {
            if c.is_ascii() {
                c.to_ascii_uppercase()
            } else {
                c
            }
        })
        .collect();
    alloc_gc_cstring(&out)
}

/// `__string_to_lower(s) -> String` — ASCII-only lower-casing.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tyra_string_to_lower(s: *const c_char) -> *const c_char {
    let out: String = borrow_utf8(s)
        .chars()
        .map(|c| {
            if c.is_ascii() {
                c.to_ascii_lowercase()
            } else {
                c
            }
        })
        .collect();
    alloc_gc_cstring(&out)
}

/// `__string_contains(s, needle) -> Bool`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tyra_string_contains(s: *const c_char, needle: *const c_char) -> c_int {
    if borrow_utf8(s).contains(borrow_utf8(needle)) {
        1
    } else {
        0
    }
}

/// `__string_starts_with(s, prefix) -> Bool`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tyra_string_starts_with(s: *const c_char, prefix: *const c_char) -> c_int {
    if borrow_utf8(s).starts_with(borrow_utf8(prefix)) {
        1
    } else {
        0
    }
}

/// `__string_ends_with(s, suffix) -> Bool`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tyra_string_ends_with(s: *const c_char, suffix: *const c_char) -> c_int {
    if borrow_utf8(s).ends_with(borrow_utf8(suffix)) {
        1
    } else {
        0
    }
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

/// `__string_byte_at(s, index) -> Int` — UTF-8 byte at `index` (0..=255),
/// or -1 when `index` is out of `[0, len(s))`. The Tyra-side wrapper
/// converts -1 to `None`.
///
/// # Safety
/// `s` must be null-terminated UTF-8 (or null).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tyra_string_byte_at(s: *const c_char, index: i64) -> i64 {
    let bytes = borrow_utf8(s).as_bytes();
    if index < 0 {
        return -1;
    }
    let idx = index as usize;
    if idx >= bytes.len() {
        return -1;
    }
    bytes[idx] as i64
}

/// `__string_substring(s, start, end) -> String` — byte-level half-open
/// slice `[start, end)`. Both bounds are clamped to `[0, len(s)]` and
/// the result is empty when `start >= end` after clamping.
///
/// v0.1 is byte-level: slicing in the middle of a multi-byte UTF-8
/// sequence yields an invalid-UTF-8 buffer which is then coerced to an
/// empty string by `alloc_gc_cstring`'s interior-NUL stripping. Callers should respect
/// code-point boundaries themselves until a grapheme-aware API lands.
///
/// # Safety
/// `s` must be null-terminated UTF-8 (or null).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tyra_string_substring(
    s: *const c_char,
    start: i64,
    end: i64,
) -> *const c_char {
    let input = borrow_utf8(s);
    let len = input.len() as i64;
    let lo = start.clamp(0, len) as usize;
    let hi = end.clamp(0, len) as usize;
    if lo >= hi {
        return alloc_gc_cstring("");
    }
    let bytes = &input.as_bytes()[lo..hi];
    // If the byte slice is not valid UTF-8 (mid-codepoint cut), fall back
    // to an empty string — v0.1 keeps the result well-formed.
    match std::str::from_utf8(bytes) {
        Ok(s) => alloc_gc_cstring(s),
        Err(_) => alloc_gc_cstring(""),
    }
}

/// `__string_reverse(s) -> String` — byte-level reverse. Not
/// grapheme-aware; reversing multi-byte UTF-8 strings yields invalid
/// UTF-8, in which case the result is an empty string.
///
/// # Safety
/// `s` must be null-terminated UTF-8 (or null).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tyra_string_reverse(s: *const c_char) -> *const c_char {
    let input = borrow_utf8(s);
    let mut bytes = input.as_bytes().to_vec();
    bytes.reverse();
    match String::from_utf8(bytes) {
        Ok(s) => alloc_gc_cstring(&s),
        Err(_) => alloc_gc_cstring(""),
    }
}

/// `__string_from_byte(b) -> String` — build a single-byte string from
/// an Int. Higher bits are truncated (only bits 0..=7 are used). Not
/// UTF-8-validated — values in `0x80..=0xFF` yield an invalid-UTF-8
/// buffer, in which case the result is an empty string.
#[unsafe(no_mangle)]
pub extern "C" fn tyra_string_from_byte(b: i64) -> *const c_char {
    let byte = (b & 0xFF) as u8;
    match String::from_utf8(vec![byte]) {
        Ok(s) => alloc_gc_cstring(&s),
        Err(_) => alloc_gc_cstring(""),
    }
}

/// Layout shared with codegen for List<String>-returning intrinsics.
/// Mirrors `%struct.List__String = type { ptr, i64 }` in LLVM IR. The
/// caller alloca's a 16-byte slot, passes its address, and we fill in
/// (data, len). String entries use alloc_gc_cstring (GC_malloc_atomic).
/// The pointer array uses GC_malloc so the collector scans it and keeps
/// the referenced strings alive for the lifetime of the list.
#[repr(C)]
pub struct ListStringRet {
    data: *mut *const c_char,
    len: i64,
}

unsafe fn fill_list_string_ret(out: *mut ListStringRet, parts: Vec<*const c_char>) {
    if out.is_null() {
        return;
    }
    let len = parts.len();
    // Use GC_malloc (not atomic) so the collector scans this array for
    // interior pointers. Each element is a *const c_char produced by
    // alloc_gc_cstring; without scanning, those strings would have no
    // traceable reference and could be collected while the list is live.
    let (data, final_len) = if len == 0 {
        (std::ptr::null_mut(), 0i64)
    } else {
        let buf =
            crate::gc::malloc(len * std::mem::size_of::<*const c_char>()) as *mut *const c_char;
        if buf.is_null() {
            // Allocation failure: return empty list to avoid null-data/len>0
            // inconsistency that would cause callers to dereference null.
            (std::ptr::null_mut(), 0i64)
        } else {
            unsafe { std::ptr::copy_nonoverlapping(parts.as_ptr(), buf, len) };
            (buf, len as i64)
        }
    };
    unsafe {
        (*out).data = data;
        (*out).len = final_len;
    }
}

/// `__string_split_whitespace(s, out)` — fills `out` with a List<String>
/// containing each maximal non-whitespace run in `s`. Whitespace follows
/// Rust's `char::is_whitespace`. Empty / whitespace-only input → empty
/// list.
///
/// # Safety
/// `s` must be null-terminated UTF-8 (or null). `out` must point at a
/// valid 16-byte (List<String>) slot.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tyra_string_split_whitespace(s: *const c_char, out: *mut ListStringRet) {
    let input = borrow_utf8(s);
    let parts: Vec<*const c_char> = input.split_whitespace().map(alloc_gc_cstring).collect();
    unsafe { fill_list_string_ret(out, parts) };
}

/// `__string_split(s, sep, out)` — fills `out` with a List<String>
/// containing the parts of `s` separated by `sep`. Empty `sep` falls
/// back to `[s]` (Tyra v0.1 does not split between every character).
/// Adjacent separators yield empty-string entries, matching Rust's
/// `str::split`.
///
/// # Safety
/// `s` and `sep` must be null-terminated UTF-8 (or null). `out` must
/// point at a valid 16-byte slot.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tyra_string_split(
    s: *const c_char,
    sep: *const c_char,
    out: *mut ListStringRet,
) {
    let input = borrow_utf8(s);
    let separator = borrow_utf8(sep);
    let parts: Vec<*const c_char> = if separator.is_empty() {
        vec![alloc_gc_cstring(input)]
    } else {
        input.split(separator).map(alloc_gc_cstring).collect()
    };
    unsafe { fill_list_string_ret(out, parts) };
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::CString;

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
        assert_eq!(
            unsafe { tyra_string_starts_with(s.as_ptr(), hello.as_ptr()) },
            1
        );
        let worl = cs("worl");
        assert_eq!(
            unsafe { tyra_string_starts_with(s.as_ptr(), worl.as_ptr()) },
            0
        );
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

    #[test]
    fn byte_at_ascii_utf8_and_out_of_range() {
        let ascii = cs("abc");
        assert_eq!(
            unsafe { tyra_string_byte_at(ascii.as_ptr(), 0) },
            b'a' as i64
        );
        assert_eq!(
            unsafe { tyra_string_byte_at(ascii.as_ptr(), 2) },
            b'c' as i64
        );
        // Out-of-range (>= len) and negative both return -1.
        assert_eq!(unsafe { tyra_string_byte_at(ascii.as_ptr(), 3) }, -1);
        assert_eq!(unsafe { tyra_string_byte_at(ascii.as_ptr(), -1) }, -1);
        // UTF-8: "あ" is 3 bytes (E3 81 82).
        let ja = cs("あ");
        assert_eq!(unsafe { tyra_string_byte_at(ja.as_ptr(), 0) }, 0xE3);
        assert_eq!(unsafe { tyra_string_byte_at(ja.as_ptr(), 1) }, 0x81);
        assert_eq!(unsafe { tyra_string_byte_at(ja.as_ptr(), 2) }, 0x82);
        assert_eq!(unsafe { tyra_string_byte_at(ja.as_ptr(), 3) }, -1);
        // Empty string yields -1 for any index.
        let empty = cs("");
        assert_eq!(unsafe { tyra_string_byte_at(empty.as_ptr(), 0) }, -1);
    }

    #[test]
    fn substring_clamps_and_slices_bytewise() {
        let s = cs("hello");
        let p = unsafe { tyra_string_substring(s.as_ptr(), 1, 4) };
        assert_eq!(unsafe { CStr::from_ptr(p) }.to_str().unwrap(), "ell");
        // Clamp end to len.
        let p = unsafe { tyra_string_substring(s.as_ptr(), 3, 100) };
        assert_eq!(unsafe { CStr::from_ptr(p) }.to_str().unwrap(), "lo");
        // Negative start clamps to 0.
        let p = unsafe { tyra_string_substring(s.as_ptr(), -5, 2) };
        assert_eq!(unsafe { CStr::from_ptr(p) }.to_str().unwrap(), "he");
        // start >= end → empty.
        let p = unsafe { tyra_string_substring(s.as_ptr(), 3, 3) };
        assert_eq!(unsafe { CStr::from_ptr(p) }.to_str().unwrap(), "");
        let p = unsafe { tyra_string_substring(s.as_ptr(), 4, 2) };
        assert_eq!(unsafe { CStr::from_ptr(p) }.to_str().unwrap(), "");
        // UTF-8 codepoint-aligned slice: "あい" bytes [0..3) = "あ".
        let ja = cs("あい");
        let p = unsafe { tyra_string_substring(ja.as_ptr(), 0, 3) };
        assert_eq!(unsafe { CStr::from_ptr(p) }.to_str().unwrap(), "あ");
        // Mid-codepoint cut is v0.1-undefined: we coerce to empty.
        let p = unsafe { tyra_string_substring(ja.as_ptr(), 0, 2) };
        assert_eq!(unsafe { CStr::from_ptr(p) }.to_str().unwrap(), "");
        // Empty input is always empty.
        let empty = cs("");
        let p = unsafe { tyra_string_substring(empty.as_ptr(), 0, 10) };
        assert_eq!(unsafe { CStr::from_ptr(p) }.to_str().unwrap(), "");
    }

    #[test]
    fn reverse_bytewise_ascii_and_utf8() {
        let s = cs("hello");
        let p = unsafe { tyra_string_reverse(s.as_ptr()) };
        assert_eq!(unsafe { CStr::from_ptr(p) }.to_str().unwrap(), "olleh");
        // Empty is empty.
        let empty = cs("");
        let p = unsafe { tyra_string_reverse(empty.as_ptr()) };
        assert_eq!(unsafe { CStr::from_ptr(p) }.to_str().unwrap(), "");
        // Byte-reversing a multi-byte UTF-8 string breaks the encoding;
        // v0.1 coerces to empty.
        let ja = cs("あ");
        let p = unsafe { tyra_string_reverse(ja.as_ptr()) };
        assert_eq!(unsafe { CStr::from_ptr(p) }.to_str().unwrap(), "");
    }

    #[test]
    fn from_byte_ascii_truncation_and_non_ascii() {
        let p = tyra_string_from_byte(b'A' as i64);
        assert_eq!(unsafe { CStr::from_ptr(p) }.to_str().unwrap(), "A");
        // Higher bits truncated: 0x141 & 0xFF = 0x41 = 'A'.
        let p = tyra_string_from_byte(0x141);
        assert_eq!(unsafe { CStr::from_ptr(p) }.to_str().unwrap(), "A");
        // 0x00 would produce a NUL byte; alloc_gc_cstring truncates at NUL
        // so the result is empty.
        let p = tyra_string_from_byte(0);
        assert_eq!(unsafe { CStr::from_ptr(p) }.to_str().unwrap(), "");
        // Non-ASCII byte (0x80..=0xFF) is not valid UTF-8 by itself →
        // empty string in v0.1.
        let p = tyra_string_from_byte(0xE3);
        assert_eq!(unsafe { CStr::from_ptr(p) }.to_str().unwrap(), "");
    }

    fn read_list(out: &ListStringRet) -> Vec<String> {
        let mut v = Vec::with_capacity(out.len as usize);
        for i in 0..out.len as isize {
            let p = unsafe { *out.data.offset(i) };
            v.push(unsafe { CStr::from_ptr(p) }.to_str().unwrap().to_string());
        }
        v
    }

    #[test]
    fn split_whitespace_collapses_runs() {
        let mut out = ListStringRet {
            data: std::ptr::null_mut(),
            len: 0,
        };
        unsafe {
            tyra_string_split_whitespace(cs("  hello  world\t\n").as_ptr(), &mut out);
        }
        assert_eq!(read_list(&out), vec!["hello", "world"]);
        // Empty input → empty list.
        let mut out = ListStringRet {
            data: std::ptr::null_mut(),
            len: 0,
        };
        unsafe {
            tyra_string_split_whitespace(cs("").as_ptr(), &mut out);
        }
        assert_eq!(out.len, 0);
    }

    #[test]
    fn split_separator_keeps_empties() {
        let mut out = ListStringRet {
            data: std::ptr::null_mut(),
            len: 0,
        };
        unsafe {
            tyra_string_split(cs("a,b,,c").as_ptr(), cs(",").as_ptr(), &mut out);
        }
        assert_eq!(read_list(&out), vec!["a", "b", "", "c"]);
        // Empty separator → single-element list of the original input.
        let mut out = ListStringRet {
            data: std::ptr::null_mut(),
            len: 0,
        };
        unsafe {
            tyra_string_split(cs("hi").as_ptr(), cs("").as_ptr(), &mut out);
        }
        assert_eq!(read_list(&out), vec!["hi"]);
    }
}
