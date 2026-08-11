//! GC-managed C string allocation for stdlib FFI return values.
//!
//! `alloc_gc_cstring` allocates a NUL-terminated byte buffer via
//! `GC_malloc_atomic` so the Boehm GC can reclaim it automatically.
//! All stdlib functions that return `*const c_char` to Tyra-compiled
//! code must use this helper instead of `CString::into_raw` (which
//! leaks via the system allocator).
//!
//! `GC_malloc_atomic` is the correct variant: the buffer contains no
//! GC-managed interior pointers, only raw bytes, so the collector does
//! not need to scan it for references.

use crate::gc::malloc_atomic;
use std::os::raw::c_char;
use std::ptr;

/// Allocate a GC-managed, NUL-terminated copy of `s`. Returns `None` on
/// `GC_malloc_atomic` allocation failure so OOM-sensitive callers (JSON
/// tree conversion, HTTP response allocation) can propagate the failure
/// through their existing `Option`/error paths instead of silently
/// substituting an empty string. Interior NUL bytes in `s` are stripped
/// (truncated at the first one) so the result is always a valid C string.
///
/// # Safety
/// The returned pointer is valid until the GC collects the buffer
/// (i.e. until no live GC root references it). Callers must not free
/// the pointer with `CString::from_raw` or any system-allocator free.
pub(crate) fn try_alloc_gc_cstring(s: &str) -> Option<*const c_char> {
    // Strip interior NULs: truncate at the first 0x00 byte.
    let bytes = s.as_bytes();
    let len = bytes.iter().position(|&b| b == 0).unwrap_or(bytes.len());
    let total = len + 1; // +1 for NUL terminator

    let buf = malloc_atomic(total) as *mut u8;
    if buf.is_null() {
        return None;
    }
    unsafe {
        ptr::copy_nonoverlapping(bytes.as_ptr(), buf, len);
        *buf.add(len) = 0;
    }
    Some(buf as *const c_char)
}

/// Allocate a GC-managed, NUL-terminated copy of `s`.
///
/// Wrapper over `try_alloc_gc_cstring` preserving the original contract
/// for callers (fs/string stdlib) that have no OOM-propagation path of
/// their own: on allocation failure, returns a pointer to a statically
/// allocated empty C string (never null) rather than `None`.
///
/// # Safety
/// The returned pointer is valid until the GC collects the buffer
/// (i.e. until no live GC root references it). Callers must not free
/// the pointer with `CString::from_raw` or any system-allocator free.
pub(crate) fn alloc_gc_cstring(s: &str) -> *const c_char {
    static EMPTY: &[u8] = b"\0";
    try_alloc_gc_cstring(s).unwrap_or(EMPTY.as_ptr() as *const c_char)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::CStr;

    fn init_gc() {
        // Use tyra_rt_init which is guarded by Once to prevent double-init.
        crate::tyra_rt_init();
    }

    #[test]
    fn roundtrip_ascii() {
        init_gc();
        let p = alloc_gc_cstring("hello");
        let s = unsafe { CStr::from_ptr(p) }.to_str().unwrap();
        assert_eq!(s, "hello");
    }

    #[test]
    fn empty_string() {
        init_gc();
        let p = alloc_gc_cstring("");
        let s = unsafe { CStr::from_ptr(p) }.to_str().unwrap();
        assert_eq!(s, "");
    }

    #[test]
    fn interior_nul_truncated() {
        init_gc();
        let p = alloc_gc_cstring("ab\0cd");
        let s = unsafe { CStr::from_ptr(p) }.to_str().unwrap();
        assert_eq!(s, "ab");
    }

    #[test]
    fn utf8_preserved() {
        init_gc();
        let p = alloc_gc_cstring("日本語");
        let s = unsafe { CStr::from_ptr(p) }.to_str().unwrap();
        assert_eq!(s, "日本語");
    }
}
