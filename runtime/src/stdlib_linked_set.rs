//! LinkedSet<T> — insertion-order-preserving persistent set (ADR-0019).
//!
//! Implemented as a thin wrapper over `TyraLinkedMap<T, ()>`.
//! The "value" slot is always a sentinel null pointer; only the key matters.
//!
//! All operations return a NEW TyraLinkedSet object (immutable-by-construction).
//! ABI mirrors `tyra_set_*` in stdlib_set.rs.

#![allow(unsafe_op_in_unsafe_fn)]

use crate::stdlib_linked_map::{
    TyraLinkedMap, tyra_linked_map_contains_key, tyra_linked_map_for_each, tyra_linked_map_insert,
    tyra_linked_map_len, tyra_linked_map_new, tyra_linked_map_remove,
};
use std::ffi::c_void;
use std::os::raw::c_int;
use std::ptr;

unsafe extern "C" {
    fn GC_malloc(size: usize) -> *mut c_void;
    fn GC_init();
}

type EqFn = unsafe extern "C" fn(*const u8, *const u8) -> i32;
type HashFn = unsafe extern "C" fn(*const u8) -> i64;

/// Public LinkedSet handle (C ABI).
/// Wraps a TyraLinkedMap where the value is always null (sentinel).
#[repr(C)]
pub struct TyraLinkedSet {
    inner: *mut TyraLinkedMap,
}

unsafe fn gc_alloc<T>() -> *mut T {
    GC_malloc(size_of::<T>()) as *mut T
}

unsafe fn wrap(inner: *mut TyraLinkedMap) -> *mut TyraLinkedSet {
    let set = gc_alloc::<TyraLinkedSet>();
    set.write(TyraLinkedSet { inner });
    set
}

/// Sentinel "unit" value used as the map value for all set elements.
/// Must be non-null so that `tyra_linked_map_get` can distinguish "present
/// with unit value" from "absent (null)". We use a static byte as the
/// sentinel address; the pointer is never dereferenced by the caller.
static UNIT_SENTINEL: u8 = 0;
#[inline]
fn unit_val() -> *const u8 {
    &raw const UNIT_SENTINEL
}

// ── Public API ────────────────────────────────────────────────────────────────

/// Create an empty LinkedSet.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tyra_linked_set_new(eq_fn: EqFn, hash_fn: HashFn) -> *mut TyraLinkedSet {
    GC_init();
    let inner = tyra_linked_map_new(eq_fn, hash_fn);
    wrap(inner)
}

/// Insert element. Returns a NEW TyraLinkedSet.
/// Inserting an already-present element is idempotent (order unchanged).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tyra_linked_set_insert(
    set: *const TyraLinkedSet,
    key: *const u8,
) -> *mut TyraLinkedSet {
    if set.is_null() {
        return ptr::null_mut();
    }
    let new_inner = tyra_linked_map_insert((*set).inner, key, unit_val());
    wrap(new_inner)
}

/// Remove element. Returns a NEW TyraLinkedSet.
/// Delegates to `tyra_linked_map_remove`; cost mirrors LinkedMap (tombstone model):
///   - element absent:  O(1) — only the wrapper struct is freshly allocated.
///   - element present: O(entries_cap + idx_cap) — entry tombstoned; compacted on
///     the next `insert`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tyra_linked_set_remove(
    set: *const TyraLinkedSet,
    key: *const u8,
) -> *mut TyraLinkedSet {
    if set.is_null() {
        return ptr::null_mut();
    }
    let new_inner = tyra_linked_map_remove((*set).inner, key);
    wrap(new_inner)
}

/// Returns 1 if `key` is in the set, 0 otherwise.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tyra_linked_set_contains(
    set: *const TyraLinkedSet,
    key: *const u8,
) -> c_int {
    if set.is_null() {
        return 0;
    }
    tyra_linked_map_contains_key((*set).inner, key)
}

/// Returns the number of elements in the set.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tyra_linked_set_len(set: *const TyraLinkedSet) -> i64 {
    if set.is_null() {
        return 0;
    }
    tyra_linked_map_len((*set).inner)
}

/// Traverse every element in insertion order, calling `callback(ctx, elem)`.
///
/// `ctx` is an opaque pointer forwarded unchanged to every callback invocation
/// (typically a pointer to a GC-managed closure environment struct).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tyra_linked_set_for_each(
    set: *const TyraLinkedSet,
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
    tyra_linked_map_for_each(
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

    unsafe extern "C" fn eq_i64(a: *const u8, b: *const u8) -> i32 {
        let va = *(a as *const i64);
        let vb = *(b as *const i64);
        (va == vb) as i32
    }

    unsafe extern "C" fn hash_i64(a: *const u8) -> i64 {
        let v = *(a as *const i64);
        v.wrapping_mul(6364136223846793005u64 as i64)
    }

    fn box_i64(v: i64) -> *const u8 {
        Box::leak(Box::new(v)) as *mut i64 as *const u8
    }

    // ── Basic correctness ────────────────────────────────────────────────────

    #[test]
    fn empty_set_len_zero() {
        let s = unsafe { tyra_linked_set_new(eq_i64, hash_i64) };
        assert_eq!(unsafe { tyra_linked_set_len(s) }, 0);
    }

    #[test]
    fn empty_set_contains_false() {
        let s = unsafe { tyra_linked_set_new(eq_i64, hash_i64) };
        assert_eq!(unsafe { tyra_linked_set_contains(s, box_i64(1)) }, 0);
    }

    #[test]
    fn insert_then_contains() {
        let s = unsafe { tyra_linked_set_new(eq_i64, hash_i64) };
        let s = unsafe { tyra_linked_set_insert(s, box_i64(42)) };
        assert_eq!(unsafe { tyra_linked_set_contains(s, box_i64(42)) }, 1);
    }

    #[test]
    fn missing_not_contained() {
        let s = unsafe { tyra_linked_set_new(eq_i64, hash_i64) };
        let s = unsafe { tyra_linked_set_insert(s, box_i64(10)) };
        assert_eq!(unsafe { tyra_linked_set_contains(s, box_i64(99)) }, 0);
    }

    #[test]
    fn duplicate_insert_idempotent() {
        let s = unsafe { tyra_linked_set_new(eq_i64, hash_i64) };
        let k = box_i64(5);
        let s = unsafe { tyra_linked_set_insert(s, k) };
        let s = unsafe { tyra_linked_set_insert(s, k) };
        assert_eq!(unsafe { tyra_linked_set_len(s) }, 1);
    }

    #[test]
    fn len_tracks_distinct_elements() {
        let s = unsafe { tyra_linked_set_new(eq_i64, hash_i64) };
        let s = unsafe { tyra_linked_set_insert(s, box_i64(1)) };
        let s = unsafe { tyra_linked_set_insert(s, box_i64(2)) };
        let s = unsafe { tyra_linked_set_insert(s, box_i64(3)) };
        assert_eq!(unsafe { tyra_linked_set_len(s) }, 3);
        let s = unsafe { tyra_linked_set_insert(s, box_i64(1)) };
        assert_eq!(unsafe { tyra_linked_set_len(s) }, 3);
    }

    // ── Insertion-order tests ────────────────────────────────────────────────

    #[test]
    fn insertion_order_preserved() {
        thread_local! {
            static ELEMS: RefCell<Vec<i64>> = RefCell::new(Vec::new());
        }

        unsafe extern "C" fn collect(ctx: *mut c_void, key: *const u8) {
            let _ = ctx;
            ELEMS.with(|c| c.borrow_mut().push(*(key as *const i64)));
        }

        let s = unsafe { tyra_linked_set_new(eq_i64, hash_i64) };
        let s = unsafe { tyra_linked_set_insert(s, box_i64(10)) };
        let s = unsafe { tyra_linked_set_insert(s, box_i64(20)) };
        let s = unsafe { tyra_linked_set_insert(s, box_i64(30)) };

        ELEMS.with(|c| c.borrow_mut().clear());
        unsafe { tyra_linked_set_for_each(s, ptr::null_mut(), collect) };
        ELEMS.with(|c| {
            assert_eq!(
                c.borrow().clone(),
                vec![10i64, 20, 30],
                "insertion order must be preserved"
            );
        });
    }

    #[test]
    fn remove_preserves_order() {
        thread_local! {
            static ELEMS2: RefCell<Vec<i64>> = RefCell::new(Vec::new());
        }

        unsafe extern "C" fn collect2(ctx: *mut c_void, key: *const u8) {
            let _ = ctx;
            ELEMS2.with(|c| c.borrow_mut().push(*(key as *const i64)));
        }

        let s = unsafe { tyra_linked_set_new(eq_i64, hash_i64) };
        let s = unsafe { tyra_linked_set_insert(s, box_i64(10)) };
        let s = unsafe { tyra_linked_set_insert(s, box_i64(20)) };
        let s = unsafe { tyra_linked_set_insert(s, box_i64(30)) };
        let s2 = unsafe { tyra_linked_set_remove(s, box_i64(20)) };

        ELEMS2.with(|c| c.borrow_mut().clear());
        unsafe { tyra_linked_set_for_each(s2, ptr::null_mut(), collect2) };
        ELEMS2.with(|c| {
            assert_eq!(
                c.borrow().clone(),
                vec![10i64, 30],
                "remaining elements must preserve original order"
            );
        });
    }

    #[test]
    fn remove_missing_element_preserves_count() {
        let s = unsafe { tyra_linked_set_new(eq_i64, hash_i64) };
        let s = unsafe { tyra_linked_set_insert(s, box_i64(42)) };
        let s2 = unsafe { tyra_linked_set_remove(s, box_i64(99)) };
        assert_eq!(unsafe { tyra_linked_set_len(s2) }, 1);
    }

    #[test]
    fn immutability_insert_does_not_mutate_original() {
        let s0 = unsafe { tyra_linked_set_new(eq_i64, hash_i64) };
        let s1 = unsafe { tyra_linked_set_insert(s0, box_i64(1)) };
        let s2 = unsafe { tyra_linked_set_insert(s1, box_i64(2)) };

        assert_eq!(unsafe { tyra_linked_set_len(s1) }, 1);
        assert_eq!(unsafe { tyra_linked_set_contains(s1, box_i64(2)) }, 0);

        assert_eq!(unsafe { tyra_linked_set_len(s2) }, 2);
        assert_eq!(unsafe { tyra_linked_set_contains(s2, box_i64(1)) }, 1);
        assert_eq!(unsafe { tyra_linked_set_contains(s2, box_i64(2)) }, 1);
    }

    // ── Tombstone / absent-remove tests ─────────────────────────────────────

    #[test]
    fn remove_absent_preserves_all_elements() {
        // O(1) absent-remove must not drop any existing element.
        let s = unsafe { tyra_linked_set_new(eq_i64, hash_i64) };
        let s = unsafe { tyra_linked_set_insert(s, box_i64(42)) };
        let s2 = unsafe { tyra_linked_set_remove(s, box_i64(99)) };
        assert_eq!(unsafe { tyra_linked_set_len(s2) }, 1);
        assert_eq!(unsafe { tyra_linked_set_contains(s2, box_i64(42)) }, 1);
        assert_eq!(unsafe { tyra_linked_set_contains(s2, box_i64(99)) }, 0);
    }

    #[test]
    fn insert_after_remove_compacts_tombstone() {
        // After a present-remove a subsequent insert should compact the tombstone
        // so that for_each emits exactly the live elements in insertion order.
        thread_local! {
            static ELEMS3: RefCell<Vec<i64>> = RefCell::new(Vec::new());
        }
        unsafe extern "C" fn collect3(ctx: *mut c_void, key: *const u8) {
            let _ = ctx;
            ELEMS3.with(|c| c.borrow_mut().push(*(key as *const i64)));
        }

        let s = unsafe { tyra_linked_set_new(eq_i64, hash_i64) };
        let s = unsafe { tyra_linked_set_insert(s, box_i64(1)) };
        let s = unsafe { tyra_linked_set_insert(s, box_i64(2)) };
        let s = unsafe { tyra_linked_set_insert(s, box_i64(3)) };
        let s = unsafe { tyra_linked_set_remove(s, box_i64(2)) }; // tombstone
        let s = unsafe { tyra_linked_set_insert(s, box_i64(4)) }; // triggers compaction

        assert_eq!(unsafe { tyra_linked_set_len(s) }, 3);
        assert_eq!(unsafe { tyra_linked_set_contains(s, box_i64(2)) }, 0);

        ELEMS3.with(|c| c.borrow_mut().clear());
        unsafe { tyra_linked_set_for_each(s, ptr::null_mut(), collect3) };
        ELEMS3.with(|c| {
            assert_eq!(
                c.borrow().clone(),
                vec![1i64, 3, 4],
                "post-compaction order must be insertion order of live elements"
            );
        });
    }

    #[test]
    fn grow_beyond_initial_capacity() {
        let s = unsafe { tyra_linked_set_new(eq_i64, hash_i64) };
        let mut cur = s;
        for i in 0..32i64 {
            cur = unsafe { tyra_linked_set_insert(cur, box_i64(i)) };
        }
        assert_eq!(unsafe { tyra_linked_set_len(cur) }, 32);
        for i in 0..32i64 {
            assert_eq!(
                unsafe { tyra_linked_set_contains(cur, box_i64(i)) },
                1,
                "element {i} missing after grow"
            );
        }
        assert_eq!(unsafe { tyra_linked_set_contains(cur, box_i64(999)) }, 0);
    }
}
