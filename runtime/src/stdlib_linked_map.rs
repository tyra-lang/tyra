//! LinkedMap<K,V> — insertion-order-preserving persistent map (ADR-0019).
//!
//! Internal structure:
//!   - `entries`: GC-managed array of (key, val) pairs in insertion order.
//!     An entry with `key == null` is a *tombstone* (logically deleted).
//!   - `index`:   GC-managed open-addressing hash table mapping key → entries index.
//!     A slot with `occupied == 2` is an index tombstone: the key was deleted
//!     but the probe chain must continue through it.
//!   - `live`:    count of non-tombstone entries (returned by `.len()`).
//!   - `entries_cap`: actual length of the `entries` array (live + tombstones).
//!
//! All operations return a NEW TyraLinkedMap object (immutable-by-construction).
//!
//! Remove amortized cost (v0.9.0, ADR-0019 §remove-O-n):
//!   - key absent:  O(1) — shared entries/index pointer, only the wrapper struct
//!     is freshly allocated.
//!   - key present: O(entries_cap + idx_cap) — entries and index are copied,
//!     one entry is tombstoned (entries) and one slot is tombstoned (index).
//!     `insert` always compacts tombstones, so entries_cap ≈ live after the next
//!     write, keeping the amortized cost low.
//!
//! ABI mirrors `tyra_map_*` in stdlib_map.rs.
//! GC safety: TyraLinkedMap is GC_malloc'd (pointer-scanning mode).

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

type EqFn = unsafe extern "C" fn(*const u8, *const u8) -> i32;
type HashFn = unsafe extern "C" fn(*const u8) -> i64;

/// A (key, val) pair stored in insertion order.
/// `key == null` marks a tombstone (logically deleted entry).
#[repr(C)]
struct Entry {
    key: *const u8,
    val: *const u8,
}

/// Open-addressing index slot.
/// `occupied` values:
///   0 = empty   — probe chain ends here (key absent)
///   1 = live    — key is present, `entry_idx` points into `entries`
///   2 = tombstone — key was deleted; probe chain must continue past this slot
#[repr(C)]
struct IndexSlot {
    key: *const u8,
    entry_idx: usize,
    occupied: u8,
}

/// Public LinkedMap handle (C ABI).
/// Allocated with GC_malloc so interior pointers are scanned by Boehm.
#[repr(C)]
pub struct TyraLinkedMap {
    eq_fn: EqFn,
    hash_fn: HashFn,
    /// Live (non-tombstone) entry count; returned by `tyra_linked_map_len`.
    live: i64,
    /// Length of the `entries` array, including tombstone slots.
    entries_cap: i64,
    /// GC-managed array of Entry, length = `entries_cap`.
    entries: *mut Entry,
    /// GC-managed open-addressing table, capacity = `idx_cap` (power of two).
    index: *mut IndexSlot,
    /// Capacity of the index table.
    idx_cap: usize,
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

// ── Index (open-addressing hash table, key → entries index) ─────────────────

/// Minimum index capacity. Must be a power of two.
const MIN_IDX_CAP: usize = 8;

fn next_pow2(n: usize) -> usize {
    let mut p = MIN_IDX_CAP;
    while p < n {
        p <<= 1;
    }
    p
}

/// Build a fresh index table from a *compact* (tombstone-free) entries array.
/// Callers guarantee that every entry in `0..entries_cap` has `key != null`.
unsafe fn build_index(
    entries: *mut Entry,
    entries_cap: usize,
    cap: usize,
    hash_fn: HashFn,
) -> *mut IndexSlot {
    let idx = gc_alloc_array::<IndexSlot>(cap);
    ptr::write_bytes(idx, 0, cap);
    for i in 0..entries_cap {
        let e = &*entries.add(i);
        debug_assert!(!e.key.is_null(), "build_index: unexpected tombstone");
        let hash = (hash_fn)(e.key) as u64;
        let mut slot = (hash as usize) & (cap - 1);
        loop {
            let s = &mut *idx.add(slot);
            if s.occupied == 0 {
                s.key = e.key;
                s.entry_idx = i;
                s.occupied = 1;
                break;
            }
            slot = (slot + 1) & (cap - 1);
        }
    }
    idx
}

/// Look up a key in the index. Returns `Some(entry_idx)` or `None`.
/// Skips tombstone slots (occupied == 2) to honour probe chain continuity.
unsafe fn index_lookup(
    index: *const IndexSlot,
    cap: usize,
    hash: u64,
    key: *const u8,
    eq_fn: EqFn,
) -> Option<usize> {
    if index.is_null() || cap == 0 {
        return None;
    }
    let mut slot = (hash as usize) & (cap - 1);
    let start = slot;
    loop {
        let s = &*index.add(slot);
        match s.occupied {
            0 => return None,
            1 if eq_fn(s.key, key) != 0 => return Some(s.entry_idx),
            _ => {
                // occupied == 2 (tombstone) or occupied == 1 with a different key
                slot = (slot + 1) & (cap - 1);
                if slot == start {
                    return None; // full table (should never happen with load < 1)
                }
            }
        }
    }
}

// ── Constructor helpers ──────────────────────────────────────────────────────

/// Build a TyraLinkedMap from a compact (tombstone-free) entries array and a
/// freshly rebuilt index.  `entries_cap` == `live` here.
unsafe fn make_map(
    eq_fn: EqFn,
    hash_fn: HashFn,
    live: usize,
    entries: *mut Entry,
) -> *mut TyraLinkedMap {
    let idx_cap = next_pow2(if live == 0 { MIN_IDX_CAP } else { live * 2 });
    let index = build_index(entries, live, idx_cap, hash_fn);
    let map = gc_alloc::<TyraLinkedMap>();
    map.write(TyraLinkedMap {
        eq_fn,
        hash_fn,
        live: live as i64,
        entries_cap: live as i64,
        entries,
        index,
        idx_cap,
    });
    map
}

// ── Public API ────────────────────────────────────────────────────────────────

/// Create an empty LinkedMap.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tyra_linked_map_new(eq_fn: EqFn, hash_fn: HashFn) -> *mut TyraLinkedMap {
    GC_init();
    let idx_cap = MIN_IDX_CAP;
    let index = gc_alloc_array::<IndexSlot>(idx_cap);
    ptr::write_bytes(index, 0, idx_cap);
    let map = gc_alloc::<TyraLinkedMap>();
    map.write(TyraLinkedMap {
        eq_fn,
        hash_fn,
        live: 0,
        entries_cap: 0,
        entries: ptr::null_mut(),
        index,
        idx_cap,
    });
    map
}

/// Insert or update `key → val`. Returns a NEW TyraLinkedMap.
///
/// Always produces a compact (tombstone-free) entries array so that subsequent
/// remove operations start from a clean baseline.
///
/// - New key: appended at the end (insertion order preserved).
/// - Existing key: value updated; position in insertion order is unchanged.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tyra_linked_map_insert(
    map: *const TyraLinkedMap,
    key: *const u8,
    val: *const u8,
) -> *mut TyraLinkedMap {
    if map.is_null() {
        return ptr::null_mut();
    }
    let m = &*map;
    let hash = (m.hash_fn)(key) as u64;
    let existing = index_lookup(m.index, m.idx_cap, hash, key, m.eq_fn);

    let old_live = m.live as usize;
    let old_entries_cap = m.entries_cap as usize;

    match existing {
        Some(entry_idx) => {
            // Key exists: compact entries (drop tombstones), update value.
            let new_entries = gc_alloc_array::<Entry>(old_live);
            let mut dst = 0usize;
            let mut new_idx = 0usize;
            for src in 0..old_entries_cap {
                let e = &*m.entries.add(src);
                if !e.key.is_null() {
                    new_entries.add(dst).write(Entry { key: e.key, val: e.val });
                    if src == entry_idx {
                        new_idx = dst;
                    }
                    dst += 1;
                }
            }
            (*new_entries.add(new_idx)).val = val;
            make_map(m.eq_fn, m.hash_fn, old_live, new_entries)
        }
        None => {
            // New key: compact existing + append.
            let new_live = old_live + 1;
            let new_entries = gc_alloc_array::<Entry>(new_live);
            let mut dst = 0usize;
            for src in 0..old_entries_cap {
                let e = &*m.entries.add(src);
                if !e.key.is_null() {
                    new_entries.add(dst).write(Entry { key: e.key, val: e.val });
                    dst += 1;
                }
            }
            new_entries.add(dst).write(Entry { key, val });
            make_map(m.eq_fn, m.hash_fn, new_live, new_entries)
        }
    }
}

/// Remove `key` from the map. Returns a NEW TyraLinkedMap.
///
/// Cost:
///   - key absent:  O(1) — only the wrapper struct is freshly allocated;
///     entries and index arrays are shared with the original map.
///   - key present: O(entries_cap + idx_cap) — entries array is copied with
///     the removed entry tombstoned (`key = null`); index is copied with the
///     corresponding slot marked tombstone (`occupied = 2`).  The next
///     `insert` call compacts tombstones back to O(live).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tyra_linked_map_remove(
    map: *const TyraLinkedMap,
    key: *const u8,
) -> *mut TyraLinkedMap {
    if map.is_null() {
        return ptr::null_mut();
    }
    let m = &*map;
    let hash = (m.hash_fn)(key) as u64;
    let existing = index_lookup(m.index, m.idx_cap, hash, key, m.eq_fn);

    match existing {
        None => {
            // Key absent: share entries and index — only allocate the wrapper.
            let new_map = gc_alloc::<TyraLinkedMap>();
            new_map.write(TyraLinkedMap {
                eq_fn: m.eq_fn,
                hash_fn: m.hash_fn,
                live: m.live,
                entries_cap: m.entries_cap,
                entries: m.entries,
                index: m.index,
                idx_cap: m.idx_cap,
            });
            new_map
        }
        Some(entry_idx) => {
            let entries_cap = m.entries_cap as usize;

            // Copy entries and tombstone the removed slot.
            let new_entries = gc_alloc_array::<Entry>(entries_cap);
            ptr::copy_nonoverlapping(m.entries, new_entries, entries_cap);
            (*new_entries.add(entry_idx)).key = ptr::null();

            // Copy index and tombstone the slot that referenced entry_idx.
            let new_index = gc_alloc_array::<IndexSlot>(m.idx_cap);
            ptr::copy_nonoverlapping(m.index, new_index, m.idx_cap);
            let mut slot = (hash as usize) & (m.idx_cap - 1);
            loop {
                let s = &mut *new_index.add(slot);
                if s.occupied == 1 && s.entry_idx == entry_idx {
                    s.occupied = 2; // index tombstone
                    break;
                }
                slot = (slot + 1) & (m.idx_cap - 1);
            }

            let new_map = gc_alloc::<TyraLinkedMap>();
            new_map.write(TyraLinkedMap {
                eq_fn: m.eq_fn,
                hash_fn: m.hash_fn,
                live: m.live - 1,
                entries_cap: entries_cap as i64,
                entries: new_entries,
                index: new_index,
                idx_cap: m.idx_cap,
            });
            new_map
        }
    }
}

/// Returns a pointer to the value box, or null if not found.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tyra_linked_map_get(
    map: *const TyraLinkedMap,
    key: *const u8,
) -> *const u8 {
    if map.is_null() {
        return ptr::null();
    }
    let m = &*map;
    let hash = (m.hash_fn)(key) as u64;
    match index_lookup(m.index, m.idx_cap, hash, key, m.eq_fn) {
        None => ptr::null(),
        Some(i) => (*m.entries.add(i)).val,
    }
}

/// Returns 1 if `key` is present, 0 otherwise.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tyra_linked_map_contains_key(
    map: *const TyraLinkedMap,
    key: *const u8,
) -> c_int {
    (!tyra_linked_map_get(map, key).is_null()) as c_int
}

/// Returns the number of live (non-tombstone) entries in the map.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tyra_linked_map_len(map: *const TyraLinkedMap) -> i64 {
    if map.is_null() {
        return 0;
    }
    (*map).live
}

/// Traverse every live entry in insertion order, calling `callback(ctx, key, val)`
/// once per entry.  Tombstone slots are skipped.
///
/// `ctx` is an opaque pointer forwarded unchanged to every callback invocation
/// (typically a pointer to a GC-managed closure environment struct).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tyra_linked_map_for_each(
    map: *const TyraLinkedMap,
    ctx: *mut c_void,
    callback: unsafe extern "C" fn(*mut c_void, *const u8, *const u8),
) {
    if map.is_null() {
        return;
    }
    let m = &*map;
    for i in 0..m.entries_cap as usize {
        let e = &*m.entries.add(i);
        if !e.key.is_null() {
            callback(ctx, e.key, e.val);
        }
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    // ── Test helpers ─────────────────────────────────────────────────────────

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

    fn unbox_i64(p: *const u8) -> i64 {
        unsafe { *(p as *const i64) }
    }

    // ── Basic correctness ────────────────────────────────────────────────────

    #[test]
    fn empty_map_get_returns_null() {
        let m = unsafe { tyra_linked_map_new(eq_i64, hash_i64) };
        assert!(unsafe { tyra_linked_map_get(m, box_i64(1)) }.is_null());
    }

    #[test]
    fn empty_map_len_zero() {
        let m = unsafe { tyra_linked_map_new(eq_i64, hash_i64) };
        assert_eq!(unsafe { tyra_linked_map_len(m) }, 0);
    }

    #[test]
    fn insert_then_get() {
        let m = unsafe { tyra_linked_map_new(eq_i64, hash_i64) };
        let m = unsafe { tyra_linked_map_insert(m, box_i64(10), box_i64(100)) };
        let got = unsafe { tyra_linked_map_get(m, box_i64(10)) };
        assert!(!got.is_null());
        assert_eq!(unbox_i64(got), 100);
    }

    #[test]
    fn test_get_missing_returns_null() {
        let m = unsafe { tyra_linked_map_new(eq_i64, hash_i64) };
        let m = unsafe { tyra_linked_map_insert(m, box_i64(10), box_i64(100)) };
        assert!(unsafe { tyra_linked_map_get(m, box_i64(99)) }.is_null());
    }

    #[test]
    fn test_len() {
        let m = unsafe { tyra_linked_map_new(eq_i64, hash_i64) };
        let m = unsafe { tyra_linked_map_insert(m, box_i64(1), box_i64(10)) };
        let m = unsafe { tyra_linked_map_insert(m, box_i64(2), box_i64(20)) };
        let m = unsafe { tyra_linked_map_insert(m, box_i64(3), box_i64(30)) };
        assert_eq!(unsafe { tyra_linked_map_len(m) }, 3);
    }

    #[test]
    fn contains_key() {
        let m = unsafe { tyra_linked_map_new(eq_i64, hash_i64) };
        let m = unsafe { tyra_linked_map_insert(m, box_i64(7), box_i64(77)) };
        assert_eq!(unsafe { tyra_linked_map_contains_key(m, box_i64(7)) }, 1);
        assert_eq!(unsafe { tyra_linked_map_contains_key(m, box_i64(8)) }, 0);
    }

    #[test]
    fn overwrite_does_not_grow_count() {
        let m = unsafe { tyra_linked_map_new(eq_i64, hash_i64) };
        let k = box_i64(5);
        let m = unsafe { tyra_linked_map_insert(m, k, box_i64(10)) };
        let m = unsafe { tyra_linked_map_insert(m, k, box_i64(20)) };
        assert_eq!(unsafe { tyra_linked_map_len(m) }, 1);
        assert_eq!(unbox_i64(unsafe { tyra_linked_map_get(m, k) }), 20);
    }

    // ── Insertion-order tests ────────────────────────────────────────────────

    #[test]
    fn test_insertion_order_preserved() {
        thread_local! {
            static COLLECTED: RefCell<Vec<i64>> = RefCell::new(Vec::new());
        }

        unsafe extern "C" fn collect_keys(ctx: *mut c_void, key: *const u8, _val: *const u8) {
            let _ = ctx;
            let v = *(key as *const i64);
            COLLECTED.with(|c| c.borrow_mut().push(v));
        }

        let m = unsafe { tyra_linked_map_new(eq_i64, hash_i64) };
        let m = unsafe { tyra_linked_map_insert(m, box_i64(1), box_i64(10)) };
        let m = unsafe { tyra_linked_map_insert(m, box_i64(2), box_i64(20)) };
        let m = unsafe { tyra_linked_map_insert(m, box_i64(3), box_i64(30)) };

        COLLECTED.with(|c| c.borrow_mut().clear());
        unsafe { tyra_linked_map_for_each(m, ptr::null_mut(), collect_keys) };
        COLLECTED.with(|c| {
            let keys = c.borrow().clone();
            assert_eq!(keys, vec![1i64, 2, 3], "insertion order must be preserved");
        });
    }

    #[test]
    fn test_remove_preserves_remaining_order() {
        thread_local! {
            static COLLECTED2: RefCell<Vec<i64>> = RefCell::new(Vec::new());
        }

        unsafe extern "C" fn collect_keys2(ctx: *mut c_void, key: *const u8, _val: *const u8) {
            let _ = ctx;
            let v = *(key as *const i64);
            COLLECTED2.with(|c| c.borrow_mut().push(v));
        }

        let m = unsafe { tyra_linked_map_new(eq_i64, hash_i64) };
        let m = unsafe { tyra_linked_map_insert(m, box_i64(1), box_i64(10)) };
        let m = unsafe { tyra_linked_map_insert(m, box_i64(2), box_i64(20)) };
        let m = unsafe { tyra_linked_map_insert(m, box_i64(3), box_i64(30)) };
        let m2 = unsafe { tyra_linked_map_remove(m, box_i64(2)) };

        COLLECTED2.with(|c| c.borrow_mut().clear());
        unsafe { tyra_linked_map_for_each(m2, ptr::null_mut(), collect_keys2) };
        COLLECTED2.with(|c| {
            let keys = c.borrow().clone();
            assert_eq!(
                keys,
                vec![1i64, 3],
                "remaining keys must preserve original order"
            );
        });
    }

    #[test]
    fn update_preserves_order() {
        thread_local! {
            static COLLECTED3: RefCell<Vec<(i64, i64)>> = RefCell::new(Vec::new());
        }

        unsafe extern "C" fn collect_kv(ctx: *mut c_void, key: *const u8, val: *const u8) {
            let _ = ctx;
            let k = *(key as *const i64);
            let v = *(val as *const i64);
            COLLECTED3.with(|c| c.borrow_mut().push((k, v)));
        }

        let m = unsafe { tyra_linked_map_new(eq_i64, hash_i64) };
        let m = unsafe { tyra_linked_map_insert(m, box_i64(1), box_i64(10)) };
        let m = unsafe { tyra_linked_map_insert(m, box_i64(2), box_i64(20)) };
        let m = unsafe { tyra_linked_map_insert(m, box_i64(3), box_i64(30)) };
        let m = unsafe { tyra_linked_map_insert(m, box_i64(2), box_i64(99)) };

        COLLECTED3.with(|c| c.borrow_mut().clear());
        unsafe { tyra_linked_map_for_each(m, ptr::null_mut(), collect_kv) };
        COLLECTED3.with(|c| {
            let pairs = c.borrow().clone();
            assert_eq!(
                pairs,
                vec![(1i64, 10i64), (2, 99), (3, 30)],
                "update must not reorder; value must be updated"
            );
        });
    }

    #[test]
    fn remove_missing_key_preserves_count() {
        let m = unsafe { tyra_linked_map_new(eq_i64, hash_i64) };
        let m = unsafe { tyra_linked_map_insert(m, box_i64(42), box_i64(1)) };
        let m2 = unsafe { tyra_linked_map_remove(m, box_i64(99)) };
        assert_eq!(unsafe { tyra_linked_map_len(m2) }, 1);
    }

    #[test]
    fn immutability_insert_does_not_mutate_original() {
        let m0 = unsafe { tyra_linked_map_new(eq_i64, hash_i64) };
        let m1 = unsafe { tyra_linked_map_insert(m0, box_i64(1), box_i64(10)) };
        let m2 = unsafe { tyra_linked_map_insert(m1, box_i64(2), box_i64(20)) };

        assert_eq!(unsafe { tyra_linked_map_len(m1) }, 1);
        assert!(unsafe { tyra_linked_map_get(m1, box_i64(2)) }.is_null());

        assert_eq!(unsafe { tyra_linked_map_len(m2) }, 2);
        assert!(!unsafe { tyra_linked_map_get(m2, box_i64(1)) }.is_null());
        assert!(!unsafe { tyra_linked_map_get(m2, box_i64(2)) }.is_null());
    }

    #[test]
    fn grow_beyond_initial_capacity() {
        let m = unsafe { tyra_linked_map_new(eq_i64, hash_i64) };
        let mut cur = m;
        for i in 0..32i64 {
            cur = unsafe { tyra_linked_map_insert(cur, box_i64(i), box_i64(i * 10)) };
        }
        assert_eq!(unsafe { tyra_linked_map_len(cur) }, 32);
        for i in 0..32i64 {
            let got = unsafe { tyra_linked_map_get(cur, box_i64(i)) };
            assert!(!got.is_null(), "key {i} missing after grow");
            assert_eq!(unbox_i64(got), i * 10);
        }
    }

    /// remove(key) → insert(key2) round-trip: insert must compact tombstones so
    /// subsequent lookups and for_each see only live entries.
    #[test]
    fn tombstone_compacted_by_subsequent_insert() {
        thread_local! {
            static KEYS: RefCell<Vec<i64>> = RefCell::new(Vec::new());
        }
        unsafe extern "C" fn collect(ctx: *mut c_void, key: *const u8, _v: *const u8) {
            let _ = ctx;
            KEYS.with(|c| c.borrow_mut().push(*(key as *const i64)));
        }

        let m = unsafe { tyra_linked_map_new(eq_i64, hash_i64) };
        let m = unsafe { tyra_linked_map_insert(m, box_i64(1), box_i64(10)) };
        let m = unsafe { tyra_linked_map_insert(m, box_i64(2), box_i64(20)) };
        let m = unsafe { tyra_linked_map_insert(m, box_i64(3), box_i64(30)) };

        // Remove key 2 → tombstone in entries/index.
        let m = unsafe { tyra_linked_map_remove(m, box_i64(2)) };
        assert_eq!(unsafe { tyra_linked_map_len(m) }, 2);
        assert!(unsafe { tyra_linked_map_get(m, box_i64(2)) }.is_null());

        // Insert key 4 → compaction occurs, tombstone disappears.
        let m = unsafe { tyra_linked_map_insert(m, box_i64(4), box_i64(40)) };
        assert_eq!(unsafe { tyra_linked_map_len(m) }, 3);

        // Verify live entries and order: 1, 3, 4.
        KEYS.with(|c| c.borrow_mut().clear());
        unsafe { tyra_linked_map_for_each(m, ptr::null_mut(), collect) };
        KEYS.with(|c| {
            assert_eq!(c.borrow().clone(), vec![1i64, 3, 4]);
        });
    }

    /// remove absent key: the returned map must be functionally equal and have
    /// correct len (O(1) path).
    #[test]
    fn remove_absent_key_returns_equivalent_map() {
        let m = unsafe { tyra_linked_map_new(eq_i64, hash_i64) };
        let m = unsafe { tyra_linked_map_insert(m, box_i64(1), box_i64(10)) };
        let m = unsafe { tyra_linked_map_insert(m, box_i64(2), box_i64(20)) };
        let m2 = unsafe { tyra_linked_map_remove(m, box_i64(99)) };

        assert_eq!(unsafe { tyra_linked_map_len(m2) }, 2);
        assert_eq!(unbox_i64(unsafe { tyra_linked_map_get(m2, box_i64(1)) }), 10);
        assert_eq!(unbox_i64(unsafe { tyra_linked_map_get(m2, box_i64(2)) }), 20);
    }

    // ── GC smoke test ────────────────────────────────────────────────────────
    //
    // Run ignored tests single-threaded to avoid Boehm GC exclusion-range
    // conflicts between parallel test threads:
    //   cargo test -p tyra-runtime -- --ignored --test-threads=1

    #[test]
    #[ignore]
    fn gc_smoke_linked_map() {
        let n = 100i64;
        let mut cur = unsafe { tyra_linked_map_new(eq_i64, hash_i64) };
        for i in 0..n {
            cur = unsafe { tyra_linked_map_insert(cur, box_i64(i), box_i64(i * 3)) };
        }
        let snapshots: Vec<*mut TyraLinkedMap> = (0..10i64)
            .map(|s| unsafe { tyra_linked_map_insert(cur, box_i64(n + s), box_i64(s * 7)) })
            .collect();

        assert_eq!(unsafe { tyra_linked_map_len(cur) }, n);
        for i in 0..n {
            let got = unsafe { tyra_linked_map_get(cur, box_i64(i)) };
            assert!(!got.is_null(), "base: key {i} missing");
            assert_eq!(unbox_i64(got), i * 3, "base: wrong value for key {i}");
        }
        for (s, &snap) in snapshots.iter().enumerate() {
            let s = s as i64;
            assert_eq!(unsafe { tyra_linked_map_len(snap) }, n + 1);
            let got = unsafe { tyra_linked_map_get(snap, box_i64(n + s)) };
            assert!(!got.is_null(), "snapshot {s}: private key missing");
            assert_eq!(unbox_i64(got), s * 7, "snapshot {s}: wrong private value");
            assert!(
                !unsafe { tyra_linked_map_get(snap, box_i64(0)) }.is_null(),
                "snapshot {s}: shared key 0 missing"
            );
        }
    }
}
