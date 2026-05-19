//! Map stdlib backing — v0.1 minimum, **Map<String, Int> only**.
//!
//! Internally a linked list of `(key, value)` nodes; lookups are linear.
//! Adequate for small lookup tables (the v0.1 use case is the
//! `string → int` configuration map shape — see ai-gen prompt 017).
//! A real hash table lands when the language gains generic V plumbing.
//!
//! Returned handles are `ptr` (`*mut MapNode`); empty map = null. All
//! nodes and key strings are allocated via the system allocator
//! (`Box::leak` / `CString::into_raw`), not `GC_malloc`, so the Boehm GC
//! never reclaims them — they leak for the process lifetime. Same
//! trade-off as fs / json.
//!
//! Concurrency: not thread-safe. v0.1 has no shared mutable state across
//! tasks for these maps; map handles are owned by the spawning task.

use std::cell::Cell;
use std::ffi::CStr;
use std::os::raw::{c_char, c_int};

thread_local! {
    /// Set to 1 when the most recent `tyra_map_get_string_int` returned
    /// a real value; 0 when the key was absent. Tyra-side `m.get(k)`
    /// reads this immediately after the value call to disambiguate
    /// missing keys from legitimate values (including `i64::MIN`).
    static MAP_GET_PRESENT: Cell<c_int> = const { Cell::new(0) };
}

#[repr(C)]
pub struct Node {
    next: *mut Node,
    key: *const c_char,   // owned (CString::into_raw)
    value: i64,
}

unsafe fn alloc_node(next: *mut Node, key: String, value: i64) -> *mut Node {
    let key_c = match std::ffi::CString::new(key) {
        Ok(c) => c.into_raw() as *const c_char,
        Err(_) => std::ffi::CString::new("").unwrap().into_raw() as *const c_char,
    };
    Box::leak(Box::new(Node { next, key: key_c, value })) as *mut Node
}

fn key_eq(node_key: *const c_char, query: &str) -> bool {
    if node_key.is_null() {
        return false;
    }
    unsafe { CStr::from_ptr(node_key) }.to_str().is_ok_and(|k| k == query)
}

/// `__map_new_string_int() -> ptr` — allocate an empty map.
#[unsafe(no_mangle)]
pub extern "C" fn tyra_map_new_string_int() -> *mut Node {
    std::ptr::null_mut()
}

/// `__map_insert_string_int(m, k, v) -> ptr` — prepend `(k, v)` and
/// return the new head. The old map handle is invalid after this call;
/// callers should always rebind. (Tyra-side map literals are immutable
/// builders so this is fine — no mutation of an in-scope binding.)
///
/// # Safety
/// `k` must be a null-terminated UTF-8 string.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tyra_map_insert_string_int(
    m: *mut Node,
    k: *const c_char,
    v: i64,
) -> *mut Node {
    let key = if k.is_null() {
        String::new()
    } else {
        unsafe { CStr::from_ptr(k) }.to_str().unwrap_or("").to_string()
    };
    unsafe { alloc_node(m, key, v) }
}

/// `__map_get_string_int(m, k) -> Int` — returns the matching value, or
/// `i64::MIN` as a sentinel for "not found". The Tyra-side wrapper
/// converts the sentinel to `None` (so legitimate `i64::MIN` values
/// would round-trip as `None` — an accepted v0.1 limitation).
///
/// # Safety
/// `k` must be a null-terminated UTF-8 string.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tyra_map_get_string_int(
    m: *mut Node,
    k: *const c_char,
) -> i64 {
    let query = if k.is_null() {
        MAP_GET_PRESENT.with(|p| p.set(0));
        return 0;
    } else {
        unsafe { CStr::from_ptr(k) }.to_str().unwrap_or("")
    };
    let mut cur = m;
    while !cur.is_null() {
        let node = unsafe { &*cur };
        if key_eq(node.key, query) {
            MAP_GET_PRESENT.with(|p| p.set(1));
            return node.value;
        }
        cur = node.next;
    }
    MAP_GET_PRESENT.with(|p| p.set(0));
    0
}

/// `__map_get_present() -> Bool` — 1 iff the most recent
/// `tyra_map_get_string_int` on this thread found the key.
#[unsafe(no_mangle)]
pub extern "C" fn tyra_map_get_present() -> c_int {
    MAP_GET_PRESENT.with(|p| p.get())
}

/// `__map_contains_string_int(m, k) -> Bool` — true iff a node with key
/// `k` exists.
///
/// # Safety
/// `k` must be a null-terminated UTF-8 string.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tyra_map_contains_string_int(
    m: *mut Node,
    k: *const c_char,
) -> c_int {
    let query = if k.is_null() {
        return 0;
    } else {
        unsafe { CStr::from_ptr(k) }.to_str().unwrap_or("")
    };
    let mut cur = m;
    while !cur.is_null() {
        let node = unsafe { &*cur };
        if key_eq(node.key, query) {
            return 1;
        }
        cur = node.next;
    }
    0
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::CString;

    fn cs(s: &str) -> CString {
        CString::new(s).unwrap()
    }

    #[test]
    fn empty_map_misses() {
        let m = tyra_map_new_string_int();
        let _ = unsafe { tyra_map_get_string_int(m, cs("k").as_ptr()) };
        assert_eq!(tyra_map_get_present(), 0);
        assert_eq!(unsafe { tyra_map_contains_string_int(m, cs("k").as_ptr()) }, 0);
    }

    #[test]
    fn insert_then_lookup() {
        let m = tyra_map_new_string_int();
        let m = unsafe { tyra_map_insert_string_int(m, cs("a").as_ptr(), 1) };
        let m = unsafe { tyra_map_insert_string_int(m, cs("b").as_ptr(), 2) };
        let m = unsafe { tyra_map_insert_string_int(m, cs("c").as_ptr(), 3) };
        assert_eq!(unsafe { tyra_map_get_string_int(m, cs("a").as_ptr()) }, 1);
        assert_eq!(unsafe { tyra_map_get_string_int(m, cs("b").as_ptr()) }, 2);
        assert_eq!(unsafe { tyra_map_get_string_int(m, cs("c").as_ptr()) }, 3);
        assert_eq!(unsafe { tyra_map_contains_string_int(m, cs("c").as_ptr()) }, 1);
        assert_eq!(unsafe { tyra_map_contains_string_int(m, cs("z").as_ptr()) }, 0);
    }

    #[test]
    fn later_insert_shadows_earlier() {
        // Linear search returns the head-most match — Tyra-side map
        // literals build by prepending, so the source-order last entry
        // for a duplicate key wins.
        let m = tyra_map_new_string_int();
        let m = unsafe { tyra_map_insert_string_int(m, cs("k").as_ptr(), 1) };
        let m = unsafe { tyra_map_insert_string_int(m, cs("k").as_ptr(), 2) };
        assert_eq!(unsafe { tyra_map_get_string_int(m, cs("k").as_ptr()) }, 2);
    }
}
