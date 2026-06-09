//! ADT display helpers for string interpolation.
//!
//! Called by compiler-generated code for `#{expr}` where `expr` has type
//! `Option<T>`. Returns GC-managed C strings for use in snprintf format args.

use crate::gc_string::alloc_gc_cstring;
use std::ffi::CStr;
use std::os::raw::c_char;

#[unsafe(no_mangle)]
pub unsafe extern "C" fn __display_option__Int(tag: i64, val: i64) -> *const c_char {
    if tag == 0 {
        alloc_gc_cstring(&format!("Some({})", val))
    } else {
        alloc_gc_cstring("None")
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn __display_option__Float(tag: i64, val: f64) -> *const c_char {
    if tag == 0 {
        alloc_gc_cstring(&format!("Some({})", val))
    } else {
        alloc_gc_cstring("None")
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn __display_option__Str(tag: i64, val: *const c_char) -> *const c_char {
    if tag == 0 {
        let s = unsafe { CStr::from_ptr(val) }.to_str().unwrap_or("<invalid>");
        alloc_gc_cstring(&format!("Some({})", s))
    } else {
        alloc_gc_cstring("None")
    }
}
