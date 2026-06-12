//! List stdlib backing (ADR-0027): sorting.
//!
//! Lists cross the FFI boundary as `{ data: ptr, len: i64 }` structs passed
//! by reference, mirroring `ListStringRet` in `stdlib_string.rs`. The output
//! array is GC-allocated: `malloc_atomic` for Int payloads (no interior
//! pointers), scanned `malloc` for String payloads (elements are GC strings
//! that must stay traceable).

use std::ffi::{c_char, CStr};

/// `List<Int>` FFI mirror: `{ data: *mut i64, len: i64 }`.
#[repr(C)]
pub struct ListI64Ret {
    data: *mut i64,
    len: i64,
}

/// `List<String>` FFI mirror: `{ data: *mut *const c_char, len: i64 }`.
#[repr(C)]
pub struct ListPtrRet {
    data: *mut *const c_char,
    len: i64,
}

/// `__list_sort(xs) -> List<Int>` — stable ascending sort into a fresh list.
///
/// # Safety
/// `input` must be null or point at a valid `{ptr,len}` List<Int> struct;
/// `out` must point at a writable 16-byte slot.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tyra_list_sort_int(input: *const ListI64Ret, out: *mut ListI64Ret) {
    if out.is_null() {
        return;
    }
    let mut v: Vec<i64> = if input.is_null() || unsafe { (*input).data.is_null() } {
        Vec::new()
    } else {
        let len = unsafe { (*input).len }.max(0) as usize;
        unsafe { std::slice::from_raw_parts((*input).data, len) }.to_vec()
    };
    v.sort(); // stable (ADR-0027)
    let (data, len) = if v.is_empty() {
        (std::ptr::null_mut(), 0i64)
    } else {
        let buf = crate::gc::malloc_atomic(v.len() * std::mem::size_of::<i64>()) as *mut i64;
        if buf.is_null() {
            (std::ptr::null_mut(), 0i64)
        } else {
            unsafe { std::ptr::copy_nonoverlapping(v.as_ptr(), buf, v.len()) };
            (buf, v.len() as i64)
        }
    };
    unsafe {
        (*out).data = data;
        (*out).len = len;
    }
}

/// `__list_sort_str(xs) -> List<String>` — stable ascending sort by UTF-8
/// byte order (the same order `SortedMap` uses for String keys; no locale
/// collation — ADR-0027).
///
/// # Safety
/// `input` must be null or point at a valid `{ptr,len}` List<String> struct
/// whose elements are NUL-terminated strings; `out` must be writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tyra_list_sort_str(input: *const ListPtrRet, out: *mut ListPtrRet) {
    if out.is_null() {
        return;
    }
    let mut v: Vec<*const c_char> = if input.is_null() || unsafe { (*input).data.is_null() } {
        Vec::new()
    } else {
        let len = unsafe { (*input).len }.max(0) as usize;
        unsafe { std::slice::from_raw_parts((*input).data, len) }.to_vec()
    };
    v.sort_by(|a, b| {
        let ab = if a.is_null() {
            &[][..]
        } else {
            unsafe { CStr::from_ptr(*a) }.to_bytes()
        };
        let bb = if b.is_null() {
            &[][..]
        } else {
            unsafe { CStr::from_ptr(*b) }.to_bytes()
        };
        ab.cmp(bb)
    });
    let (data, len) = if v.is_empty() {
        (std::ptr::null_mut(), 0i64)
    } else {
        // Scanned malloc: elements are GC strings that must stay traceable.
        let buf =
            crate::gc::malloc(v.len() * std::mem::size_of::<*const c_char>()) as *mut *const c_char;
        if buf.is_null() {
            (std::ptr::null_mut(), 0i64)
        } else {
            unsafe { std::ptr::copy_nonoverlapping(v.as_ptr(), buf, v.len()) };
            (buf, v.len() as i64)
        }
    };
    unsafe {
        (*out).data = data;
        (*out).len = len;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sort_int_sorts_and_handles_empty() {
        crate::tyra_rt_init();
        let vals = [3i64, 1, 2];
        let input = ListI64Ret {
            data: vals.as_ptr() as *mut i64,
            len: 3,
        };
        let mut out = ListI64Ret {
            data: std::ptr::null_mut(),
            len: 0,
        };
        unsafe { tyra_list_sort_int(&input, &mut out) };
        let sorted = unsafe { std::slice::from_raw_parts(out.data, out.len as usize) };
        assert_eq!(sorted, &[1, 2, 3]);

        let empty = ListI64Ret {
            data: std::ptr::null_mut(),
            len: 0,
        };
        let mut out2 = ListI64Ret {
            data: vals.as_ptr() as *mut i64,
            len: 99,
        };
        unsafe { tyra_list_sort_int(&empty, &mut out2) };
        assert_eq!(out2.len, 0);
    }

    #[test]
    fn sort_str_sorts_by_bytes() {
        crate::tyra_rt_init();
        let a = c"banana".as_ptr();
        let b = c"apple".as_ptr();
        let c = c"cherry".as_ptr();
        let vals = [a, b, c];
        let input = ListPtrRet {
            data: vals.as_ptr() as *mut *const c_char,
            len: 3,
        };
        let mut out = ListPtrRet {
            data: std::ptr::null_mut(),
            len: 0,
        };
        unsafe { tyra_list_sort_str(&input, &mut out) };
        let sorted = unsafe { std::slice::from_raw_parts(out.data, out.len as usize) };
        let first = unsafe { CStr::from_ptr(sorted[0]) }.to_str().unwrap();
        let last = unsafe { CStr::from_ptr(sorted[2]) }.to_str().unwrap();
        assert_eq!(first, "apple");
        assert_eq!(last, "cherry");
    }
}
