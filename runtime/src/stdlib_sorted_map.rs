//! SortedMap<K,V> — key-sorted persistent map (ADR-0024).
//!
//! Internal structure:
//!   - `entries`: GC-managed sorted array of (key, val) pairs.
//!   - `len`: number of live entries.
//!   - `cmp_fn`: three-way comparison `fn(a, b) -> i32`
//!     (negative = a < b, zero = equal, positive = a > b).
//!
//! All operations return a NEW TyraSortedMap (immutable-by-construction).
//! Lookup: O(log n) via binary search. Insert/remove: O(n) array copy.
//! Iteration order: ascending key order.
//!
//! Float keys are rejected at the type-checker level (ADR-0024: Float has
//! Ord but not Eq; NaN behaviour under equality is undefined).
//! GC safety: TyraSortedMap is GC_malloc'd (pointer-scanning mode).

#![allow(unsafe_op_in_unsafe_fn)]

use std::ffi::c_void;
use std::os::raw::c_int;
use std::ptr;

// ── Boehm GC extern ─────────────────────────────────────────────────────────

unsafe extern "C" {
    fn GC_malloc(size: usize) -> *mut c_void;
    fn GC_init();
}

// ── Types ────────────────────────────────────────────────────────────────────

type CmpFn = unsafe extern "C" fn(*const u8, *const u8) -> i32;

/// A (key, val) pair stored in sorted order.
#[repr(C)]
pub(crate) struct Entry {
    pub(crate) key: *const u8,
    pub(crate) val: *const u8,
}

/// Public SortedMap handle (C ABI).
/// Allocated with GC_malloc so interior pointers are scanned by Boehm.
#[repr(C)]
pub struct TyraSortedMap {
    pub(crate) cmp_fn: CmpFn,
    pub(crate) len: i64,
    /// GC-managed array of Entry, length = `len`.
    pub(crate) entries: *mut Entry,
}

// ── GC allocation helpers ────────────────────────────────────────────────────

unsafe fn gc_alloc<T>() -> *mut T {
    GC_malloc(size_of::<T>()) as *mut T
}

unsafe fn gc_alloc_array<T>(count: usize) -> *mut T {
    if count == 0 {
        return ptr::null_mut();
    }
    GC_malloc(size_of::<T>() * count) as *mut T
}

// ── Binary search ────────────────────────────────────────────────────────────

/// Binary search for `key` in the sorted entries array.
/// Returns `Ok(idx)` if found, `Err(idx)` for the insertion point.
unsafe fn binary_search(
    entries: *const Entry,
    len: usize,
    key: *const u8,
    cmp_fn: CmpFn,
) -> Result<usize, usize> {
    if len == 0 {
        return Err(0);
    }
    let mut lo = 0usize;
    let mut hi = len;
    while lo < hi {
        let mid = lo + (hi - lo) / 2;
        let c = cmp_fn((*entries.add(mid)).key, key);
        if c < 0 {
            lo = mid + 1;
        } else if c > 0 {
            hi = mid;
        } else {
            return Ok(mid);
        }
    }
    Err(lo)
}

// ── Public API ────────────────────────────────────────────────────────────────

/// Create an empty SortedMap with the given three-way key comparison function.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tyra_sorted_map_new(cmp_fn: CmpFn) -> *mut TyraSortedMap {
    GC_init();
    let map = gc_alloc::<TyraSortedMap>();
    map.write(TyraSortedMap { cmp_fn, len: 0, entries: ptr::null_mut() });
    map
}

/// Insert or update `key → val`. Returns a NEW TyraSortedMap in sorted order.
///
/// - New key: inserted at the correct sorted position.
/// - Existing key: value updated; position unchanged.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tyra_sorted_map_insert(
    map: *const TyraSortedMap,
    key: *const u8,
    val: *const u8,
) -> *mut TyraSortedMap {
    if map.is_null() {
        return ptr::null_mut();
    }
    let m = &*map;
    let old_len = m.len as usize;
    match binary_search(m.entries, old_len, key, m.cmp_fn) {
        Ok(idx) => {
            // Key exists: copy array and update the value.
            let new_entries = gc_alloc_array::<Entry>(old_len);
            ptr::copy_nonoverlapping(m.entries, new_entries, old_len);
            (*new_entries.add(idx)).val = val;
            let out = gc_alloc::<TyraSortedMap>();
            out.write(TyraSortedMap { cmp_fn: m.cmp_fn, len: m.len, entries: new_entries });
            out
        }
        Err(idx) => {
            // New key: allocate, copy prefix, insert, copy suffix.
            let new_len = old_len + 1;
            let new_entries = gc_alloc_array::<Entry>(new_len);
            if idx > 0 {
                ptr::copy_nonoverlapping(m.entries, new_entries, idx);
            }
            new_entries.add(idx).write(Entry { key, val });
            if idx < old_len {
                ptr::copy_nonoverlapping(
                    m.entries.add(idx),
                    new_entries.add(idx + 1),
                    old_len - idx,
                );
            }
            let out = gc_alloc::<TyraSortedMap>();
            out.write(TyraSortedMap {
                cmp_fn: m.cmp_fn,
                len: new_len as i64,
                entries: new_entries,
            });
            out
        }
    }
}

/// Remove `key` from the map. Returns a NEW TyraSortedMap.
///
/// - Key absent:  O(1) — only the wrapper struct is freshly allocated; the
///   entries array is shared with the original map.
/// - Key present: O(n) — entries array is copied without the removed slot.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tyra_sorted_map_remove(
    map: *const TyraSortedMap,
    key: *const u8,
) -> *mut TyraSortedMap {
    if map.is_null() {
        return ptr::null_mut();
    }
    let m = &*map;
    let old_len = m.len as usize;
    match binary_search(m.entries, old_len, key, m.cmp_fn) {
        Err(_) => {
            // Key absent: share entries, only copy the wrapper.
            let out = gc_alloc::<TyraSortedMap>();
            out.write(TyraSortedMap { cmp_fn: m.cmp_fn, len: m.len, entries: m.entries });
            out
        }
        Ok(idx) => {
            // Key present: copy everything except idx.
            let new_len = old_len - 1;
            let new_entries = if new_len == 0 {
                ptr::null_mut()
            } else {
                let e = gc_alloc_array::<Entry>(new_len);
                if idx > 0 {
                    ptr::copy_nonoverlapping(m.entries, e, idx);
                }
                if idx < new_len {
                    ptr::copy_nonoverlapping(
                        m.entries.add(idx + 1),
                        e.add(idx),
                        new_len - idx,
                    );
                }
                e
            };
            let out = gc_alloc::<TyraSortedMap>();
            out.write(TyraSortedMap {
                cmp_fn: m.cmp_fn,
                len: new_len as i64,
                entries: new_entries,
            });
            out
        }
    }
}

/// Returns a pointer to the value box, or null if not found.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tyra_sorted_map_get(
    map: *const TyraSortedMap,
    key: *const u8,
) -> *const u8 {
    if map.is_null() {
        return ptr::null();
    }
    let m = &*map;
    match binary_search(m.entries, m.len as usize, key, m.cmp_fn) {
        Ok(idx) => (*m.entries.add(idx)).val,
        Err(_) => ptr::null(),
    }
}

/// Returns 1 if `key` is present, 0 otherwise.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tyra_sorted_map_contains_key(
    map: *const TyraSortedMap,
    key: *const u8,
) -> c_int {
    (!tyra_sorted_map_get(map, key).is_null()) as c_int
}

/// Returns the number of entries in the map.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tyra_sorted_map_len(map: *const TyraSortedMap) -> i64 {
    if map.is_null() {
        return 0;
    }
    (*map).len
}

/// Traverse every entry in ascending key order, calling `callback(ctx, key, val)`.
///
/// `ctx` is an opaque pointer forwarded unchanged to every callback invocation.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tyra_sorted_map_for_each(
    map: *const TyraSortedMap,
    ctx: *mut c_void,
    callback: unsafe extern "C" fn(*mut c_void, *const u8, *const u8),
) {
    if map.is_null() {
        return;
    }
    let m = &*map;
    for i in 0..m.len as usize {
        let e = &*m.entries.add(i);
        callback(ctx, e.key, e.val);
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    // ── Test helpers ─────────────────────────────────────────────────────────

    unsafe extern "C" fn cmp_i64(a: *const u8, b: *const u8) -> i32 {
        let va = *(a as *const i64);
        let vb = *(b as *const i64);
        if va < vb { -1 } else if va > vb { 1 } else { 0 }
    }

    fn box_i64(v: i64) -> *const u8 {
        Box::leak(Box::new(v)) as *mut i64 as *const u8
    }

    fn unbox_i64(p: *const u8) -> i64 {
        unsafe { *(p as *const i64) }
    }

    // ── Basic correctness ────────────────────────────────────────────────────

    #[test]
    fn empty_map_get_returns_null() {
        let m = unsafe { tyra_sorted_map_new(cmp_i64) };
        assert!(unsafe { tyra_sorted_map_get(m, box_i64(1)) }.is_null());
    }

    #[test]
    fn empty_map_len_zero() {
        let m = unsafe { tyra_sorted_map_new(cmp_i64) };
        assert_eq!(unsafe { tyra_sorted_map_len(m) }, 0);
    }

    #[test]
    fn insert_then_get() {
        let m = unsafe { tyra_sorted_map_new(cmp_i64) };
        let m = unsafe { tyra_sorted_map_insert(m, box_i64(10), box_i64(100)) };
        let got = unsafe { tyra_sorted_map_get(m, box_i64(10)) };
        assert!(!got.is_null());
        assert_eq!(unbox_i64(got), 100);
    }

    #[test]
    fn get_missing_returns_null() {
        let m = unsafe { tyra_sorted_map_new(cmp_i64) };
        let m = unsafe { tyra_sorted_map_insert(m, box_i64(10), box_i64(100)) };
        assert!(unsafe { tyra_sorted_map_get(m, box_i64(99)) }.is_null());
    }

    #[test]
    fn len_tracks_distinct_keys() {
        let m = unsafe { tyra_sorted_map_new(cmp_i64) };
        let m = unsafe { tyra_sorted_map_insert(m, box_i64(1), box_i64(10)) };
        let m = unsafe { tyra_sorted_map_insert(m, box_i64(2), box_i64(20)) };
        let m = unsafe { tyra_sorted_map_insert(m, box_i64(3), box_i64(30)) };
        assert_eq!(unsafe { tyra_sorted_map_len(m) }, 3);
    }

    #[test]
    fn contains_key() {
        let m = unsafe { tyra_sorted_map_new(cmp_i64) };
        let m = unsafe { tyra_sorted_map_insert(m, box_i64(7), box_i64(77)) };
        assert_eq!(unsafe { tyra_sorted_map_contains_key(m, box_i64(7)) }, 1);
        assert_eq!(unsafe { tyra_sorted_map_contains_key(m, box_i64(8)) }, 0);
    }

    #[test]
    fn overwrite_does_not_grow_count() {
        let m = unsafe { tyra_sorted_map_new(cmp_i64) };
        let k = box_i64(5);
        let m = unsafe { tyra_sorted_map_insert(m, k, box_i64(10)) };
        let m = unsafe { tyra_sorted_map_insert(m, k, box_i64(20)) };
        assert_eq!(unsafe { tyra_sorted_map_len(m) }, 1);
        assert_eq!(unbox_i64(unsafe { tyra_sorted_map_get(m, k) }), 20);
    }

    // ── Sorted order tests ───────────────────────────────────────────────────

    #[test]
    fn sorted_order_after_out_of_order_inserts() {
        thread_local! {
            static KEYS: RefCell<Vec<i64>> = RefCell::new(Vec::new());
        }
        unsafe extern "C" fn collect(ctx: *mut c_void, key: *const u8, _val: *const u8) {
            let _ = ctx;
            KEYS.with(|c| c.borrow_mut().push(*(key as *const i64)));
        }

        // Insert 3, 1, 2 → expect ascending order 1, 2, 3 in for_each.
        let m = unsafe { tyra_sorted_map_new(cmp_i64) };
        let m = unsafe { tyra_sorted_map_insert(m, box_i64(3), box_i64(30)) };
        let m = unsafe { tyra_sorted_map_insert(m, box_i64(1), box_i64(10)) };
        let m = unsafe { tyra_sorted_map_insert(m, box_i64(2), box_i64(20)) };

        KEYS.with(|c| c.borrow_mut().clear());
        unsafe { tyra_sorted_map_for_each(m, ptr::null_mut(), collect) };
        KEYS.with(|c| {
            assert_eq!(
                c.borrow().clone(),
                vec![1i64, 2, 3],
                "for_each must iterate in ascending key order"
            );
        });
    }

    #[test]
    fn remove_preserves_sorted_order() {
        thread_local! {
            static KEYS2: RefCell<Vec<i64>> = RefCell::new(Vec::new());
        }
        unsafe extern "C" fn collect2(ctx: *mut c_void, key: *const u8, _val: *const u8) {
            let _ = ctx;
            KEYS2.with(|c| c.borrow_mut().push(*(key as *const i64)));
        }

        let m = unsafe { tyra_sorted_map_new(cmp_i64) };
        let m = unsafe { tyra_sorted_map_insert(m, box_i64(3), box_i64(30)) };
        let m = unsafe { tyra_sorted_map_insert(m, box_i64(1), box_i64(10)) };
        let m = unsafe { tyra_sorted_map_insert(m, box_i64(2), box_i64(20)) };
        let m2 = unsafe { tyra_sorted_map_remove(m, box_i64(2)) };

        KEYS2.with(|c| c.borrow_mut().clear());
        unsafe { tyra_sorted_map_for_each(m2, ptr::null_mut(), collect2) };
        KEYS2.with(|c| {
            assert_eq!(c.borrow().clone(), vec![1i64, 3], "remaining keys must stay sorted");
        });
    }

    #[test]
    fn remove_absent_key_preserves_count() {
        let m = unsafe { tyra_sorted_map_new(cmp_i64) };
        let m = unsafe { tyra_sorted_map_insert(m, box_i64(42), box_i64(1)) };
        let m2 = unsafe { tyra_sorted_map_remove(m, box_i64(99)) };
        assert_eq!(unsafe { tyra_sorted_map_len(m2) }, 1);
    }

    #[test]
    fn immutability_insert_does_not_mutate_original() {
        let m0 = unsafe { tyra_sorted_map_new(cmp_i64) };
        let m1 = unsafe { tyra_sorted_map_insert(m0, box_i64(1), box_i64(10)) };
        let m2 = unsafe { tyra_sorted_map_insert(m1, box_i64(2), box_i64(20)) };

        assert_eq!(unsafe { tyra_sorted_map_len(m1) }, 1);
        assert!(unsafe { tyra_sorted_map_get(m1, box_i64(2)) }.is_null());

        assert_eq!(unsafe { tyra_sorted_map_len(m2) }, 2);
        assert!(!unsafe { tyra_sorted_map_get(m2, box_i64(1)) }.is_null());
        assert!(!unsafe { tyra_sorted_map_get(m2, box_i64(2)) }.is_null());
    }

    #[test]
    fn grow_with_reverse_insertion() {
        let m = unsafe { tyra_sorted_map_new(cmp_i64) };
        let mut cur = m;
        for i in (0..32i64).rev() {
            cur = unsafe { tyra_sorted_map_insert(cur, box_i64(i), box_i64(i * 10)) };
        }
        assert_eq!(unsafe { tyra_sorted_map_len(cur) }, 32);
        for i in 0..32i64 {
            let got = unsafe { tyra_sorted_map_get(cur, box_i64(i)) };
            assert!(!got.is_null(), "key {i} missing after reverse-insert");
            assert_eq!(unbox_i64(got), i * 10);
        }
    }

    // ── GC smoke test ────────────────────────────────────────────────────────
    //
    // Run with: cargo test -p tyra-runtime -- --ignored --test-threads=1

    #[test]
    #[ignore]
    fn gc_smoke_sorted_map() {
        let n = 100i64;
        let mut cur = unsafe { tyra_sorted_map_new(cmp_i64) };
        for i in 0..n {
            cur = unsafe { tyra_sorted_map_insert(cur, box_i64(i), box_i64(i * 3)) };
        }
        assert_eq!(unsafe { tyra_sorted_map_len(cur) }, n);
        for i in 0..n {
            let got = unsafe { tyra_sorted_map_get(cur, box_i64(i)) };
            assert!(!got.is_null(), "key {i} missing");
            assert_eq!(unsafe { *(got as *const i64) }, i * 3);
        }
    }
}
