//! SortedSet<T> — key-sorted persistent set (ADR-0024).
//!
//! Implemented as a thin wrapper over `TyraSortedMap<T, ()>`.
//! The "value" slot is always a sentinel non-null pointer; only the key matters.
//!
//! All operations return a NEW TyraSortedSet object (immutable-by-construction).
//! Iteration order: ascending element order.

#![allow(unsafe_op_in_unsafe_fn)]

use crate::stdlib_sorted_map::{
    TyraSortedMap, tyra_sorted_map_contains_key, tyra_sorted_map_for_each,
    tyra_sorted_map_insert, tyra_sorted_map_len, tyra_sorted_map_new, tyra_sorted_map_remove,
};
use std::ffi::c_void;
use std::os::raw::c_int;
use std::ptr;

unsafe extern "C" {
    fn GC_malloc(size: usize) -> *mut c_void;
    fn GC_init();
}

type CmpFn = unsafe extern "C" fn(*const u8, *const u8) -> i32;

/// Public SortedSet handle (C ABI).
/// Wraps a TyraSortedMap where the value is always a sentinel non-null pointer.
#[repr(C)]
pub struct TyraSortedSet {
    inner: *mut TyraSortedMap,
}

unsafe fn gc_alloc<T>() -> *mut T {
    GC_malloc(size_of::<T>()) as *mut T
}

unsafe fn wrap(inner: *mut TyraSortedMap) -> *mut TyraSortedSet {
    let set = gc_alloc::<TyraSortedSet>();
    set.write(TyraSortedSet { inner });
    set
}

/// Sentinel "unit" value used as the map value for all set elements.
/// Must be non-null so that `tyra_sorted_map_get` can distinguish
/// "present (unit)" from "absent (null)".
static UNIT_SENTINEL: u8 = 0;
#[inline]
fn unit_val() -> *const u8 {
    &raw const UNIT_SENTINEL
}

// ── Public API ────────────────────────────────────────────────────────────────

/// Create an empty SortedSet with the given three-way element comparison function.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tyra_sorted_set_new(cmp_fn: CmpFn) -> *mut TyraSortedSet {
    GC_init();
    let inner = tyra_sorted_map_new(cmp_fn);
    wrap(inner)
}

/// Insert element. Returns a NEW TyraSortedSet.
/// Inserting an already-present element is idempotent.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tyra_sorted_set_insert(
    set: *const TyraSortedSet,
    key: *const u8,
) -> *mut TyraSortedSet {
    if set.is_null() {
        return ptr::null_mut();
    }
    let new_inner = tyra_sorted_map_insert((*set).inner, key, unit_val());
    wrap(new_inner)
}

/// Remove element. Returns a NEW TyraSortedSet.
///   - element absent:  O(1) — only the wrapper struct is freshly allocated.
///   - element present: O(n) — entries array is copied without the element.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tyra_sorted_set_remove(
    set: *const TyraSortedSet,
    key: *const u8,
) -> *mut TyraSortedSet {
    if set.is_null() {
        return ptr::null_mut();
    }
    let new_inner = tyra_sorted_map_remove((*set).inner, key);
    wrap(new_inner)
}

/// Returns 1 if `key` is in the set, 0 otherwise.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tyra_sorted_set_contains(
    set: *const TyraSortedSet,
    key: *const u8,
) -> c_int {
    if set.is_null() {
        return 0;
    }
    tyra_sorted_map_contains_key((*set).inner, key)
}

/// Returns the number of elements in the set.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tyra_sorted_set_len(set: *const TyraSortedSet) -> i64 {
    if set.is_null() {
        return 0;
    }
    tyra_sorted_map_len((*set).inner)
}

/// Traverse every element in ascending order, calling `callback(ctx, elem)`.
///
/// `ctx` is an opaque pointer forwarded unchanged to every callback invocation.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tyra_sorted_set_for_each(
    set: *const TyraSortedSet,
    ctx: *mut c_void,
    callback: unsafe extern "C" fn(*mut c_void, *const u8),
) {
    if set.is_null() {
        return;
    }

    // Adapter: the map callback receives (ctx, key, val); we forward (ctx, key).
    struct Adapter {
        ctx: *mut c_void,
        cb: unsafe extern "C" fn(*mut c_void, *const u8),
    }

    unsafe extern "C" fn map_cb(ctx: *mut c_void, key: *const u8, _val: *const u8) {
        let adapter = &*(ctx as *const Adapter);
        (adapter.cb)(adapter.ctx, key);
    }

    let adapter = Adapter { ctx, cb: callback };
    tyra_sorted_map_for_each(
        (*set).inner,
        &adapter as *const Adapter as *mut c_void,
        map_cb,
    );
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    unsafe extern "C" fn cmp_i64(a: *const u8, b: *const u8) -> i32 {
        let va = *(a as *const i64);
        let vb = *(b as *const i64);
        if va < vb { -1 } else if va > vb { 1 } else { 0 }
    }

    fn box_i64(v: i64) -> *const u8 {
        Box::leak(Box::new(v)) as *mut i64 as *const u8
    }

    // ── Basic correctness ────────────────────────────────────────────────────

    #[test]
    fn empty_set_len_zero() {
        let s = unsafe { tyra_sorted_set_new(cmp_i64) };
        assert_eq!(unsafe { tyra_sorted_set_len(s) }, 0);
    }

    #[test]
    fn empty_set_contains_false() {
        let s = unsafe { tyra_sorted_set_new(cmp_i64) };
        assert_eq!(unsafe { tyra_sorted_set_contains(s, box_i64(1)) }, 0);
    }

    #[test]
    fn insert_then_contains() {
        let s = unsafe { tyra_sorted_set_new(cmp_i64) };
        let s = unsafe { tyra_sorted_set_insert(s, box_i64(42)) };
        assert_eq!(unsafe { tyra_sorted_set_contains(s, box_i64(42)) }, 1);
    }

    #[test]
    fn missing_not_contained() {
        let s = unsafe { tyra_sorted_set_new(cmp_i64) };
        let s = unsafe { tyra_sorted_set_insert(s, box_i64(10)) };
        assert_eq!(unsafe { tyra_sorted_set_contains(s, box_i64(99)) }, 0);
    }

    #[test]
    fn duplicate_insert_idempotent() {
        let s = unsafe { tyra_sorted_set_new(cmp_i64) };
        let k = box_i64(5);
        let s = unsafe { tyra_sorted_set_insert(s, k) };
        let s = unsafe { tyra_sorted_set_insert(s, k) };
        assert_eq!(unsafe { tyra_sorted_set_len(s) }, 1);
    }

    #[test]
    fn len_tracks_distinct_elements() {
        let s = unsafe { tyra_sorted_set_new(cmp_i64) };
        let s = unsafe { tyra_sorted_set_insert(s, box_i64(1)) };
        let s = unsafe { tyra_sorted_set_insert(s, box_i64(2)) };
        let s = unsafe { tyra_sorted_set_insert(s, box_i64(3)) };
        assert_eq!(unsafe { tyra_sorted_set_len(s) }, 3);
        let s = unsafe { tyra_sorted_set_insert(s, box_i64(1)) };
        assert_eq!(unsafe { tyra_sorted_set_len(s) }, 3);
    }

    // ── Sorted order tests ───────────────────────────────────────────────────

    #[test]
    fn sorted_order_after_out_of_order_inserts() {
        thread_local! {
            static ELEMS: RefCell<Vec<i64>> = RefCell::new(Vec::new());
        }
        unsafe extern "C" fn collect(ctx: *mut c_void, key: *const u8) {
            let _ = ctx;
            ELEMS.with(|c| c.borrow_mut().push(*(key as *const i64)));
        }

        let s = unsafe { tyra_sorted_set_new(cmp_i64) };
        let s = unsafe { tyra_sorted_set_insert(s, box_i64(30)) };
        let s = unsafe { tyra_sorted_set_insert(s, box_i64(10)) };
        let s = unsafe { tyra_sorted_set_insert(s, box_i64(20)) };

        ELEMS.with(|c| c.borrow_mut().clear());
        unsafe { tyra_sorted_set_for_each(s, ptr::null_mut(), collect) };
        ELEMS.with(|c| {
            assert_eq!(
                c.borrow().clone(),
                vec![10i64, 20, 30],
                "for_each must iterate in ascending order"
            );
        });
    }

    #[test]
    fn remove_preserves_sorted_order() {
        thread_local! {
            static ELEMS2: RefCell<Vec<i64>> = RefCell::new(Vec::new());
        }
        unsafe extern "C" fn collect2(ctx: *mut c_void, key: *const u8) {
            let _ = ctx;
            ELEMS2.with(|c| c.borrow_mut().push(*(key as *const i64)));
        }

        let s = unsafe { tyra_sorted_set_new(cmp_i64) };
        let s = unsafe { tyra_sorted_set_insert(s, box_i64(30)) };
        let s = unsafe { tyra_sorted_set_insert(s, box_i64(10)) };
        let s = unsafe { tyra_sorted_set_insert(s, box_i64(20)) };
        let s2 = unsafe { tyra_sorted_set_remove(s, box_i64(20)) };

        ELEMS2.with(|c| c.borrow_mut().clear());
        unsafe { tyra_sorted_set_for_each(s2, ptr::null_mut(), collect2) };
        ELEMS2.with(|c| {
            assert_eq!(c.borrow().clone(), vec![10i64, 30], "remaining elements must stay sorted");
        });
    }

    #[test]
    fn remove_missing_element_preserves_count() {
        let s = unsafe { tyra_sorted_set_new(cmp_i64) };
        let s = unsafe { tyra_sorted_set_insert(s, box_i64(42)) };
        let s2 = unsafe { tyra_sorted_set_remove(s, box_i64(99)) };
        assert_eq!(unsafe { tyra_sorted_set_len(s2) }, 1);
    }

    #[test]
    fn immutability_insert_does_not_mutate_original() {
        let s0 = unsafe { tyra_sorted_set_new(cmp_i64) };
        let s1 = unsafe { tyra_sorted_set_insert(s0, box_i64(1)) };
        let s2 = unsafe { tyra_sorted_set_insert(s1, box_i64(2)) };

        assert_eq!(unsafe { tyra_sorted_set_len(s1) }, 1);
        assert_eq!(unsafe { tyra_sorted_set_contains(s1, box_i64(2)) }, 0);

        assert_eq!(unsafe { tyra_sorted_set_len(s2) }, 2);
        assert_eq!(unsafe { tyra_sorted_set_contains(s2, box_i64(1)) }, 1);
        assert_eq!(unsafe { tyra_sorted_set_contains(s2, box_i64(2)) }, 1);
    }

    // ── GC smoke test ────────────────────────────────────────────────────────
    //
    // Run with: cargo test -p tyra-runtime -- --ignored --test-threads=1

    #[test]
    #[ignore]
    fn gc_smoke_sorted_set() {
        let s = unsafe { tyra_sorted_set_new(cmp_i64) };
        let mut cur = s;
        for i in (0..100i64).rev() {
            cur = unsafe { tyra_sorted_set_insert(cur, box_i64(i)) };
        }
        assert_eq!(unsafe { tyra_sorted_set_len(cur) }, 100);
        for i in 0..100i64 {
            assert_eq!(
                unsafe { tyra_sorted_set_contains(cur, box_i64(i)) },
                1,
                "element {i} missing"
            );
        }
    }
}
