//! Generic Set<T> runtime — open-addressing hash set (ADR-0015).
//!
//! Elements are stored as opaque `*const u8` pointing to GC_malloc'd boxes.
//! `eq_fn` and `hash_fn` are compiler-generated per-T LLVM functions passed
//! at construction time. All allocations use Boehm GC; no manual free needed.

#![allow(unsafe_op_in_unsafe_fn)]

use std::ffi::c_void;
use std::os::raw::c_int;

unsafe extern "C" {
    fn GC_malloc(size: usize) -> *mut c_void;
}

type EqFn = unsafe extern "C" fn(*const u8, *const u8) -> i32;
type HashFn = unsafe extern "C" fn(*const u8) -> i64;

#[repr(C)]
struct TyraSetEntry {
    key: *const u8, // null → empty slot
}

#[repr(C)]
pub struct TyraSet {
    eq_fn: EqFn,
    hash_fn: HashFn,
    count: i64,
    capacity: i64, // always a power of 2
    entries: *mut TyraSetEntry,
}

const INITIAL_CAPACITY: i64 = 8;
const LOAD_FACTOR_NUM: i64 = 3;
const LOAD_FACTOR_DEN: i64 = 4;

unsafe fn alloc_entries(cap: i64) -> *mut TyraSetEntry {
    let bytes = (cap as usize) * size_of::<TyraSetEntry>();
    let ptr = unsafe { GC_malloc(bytes) } as *mut TyraSetEntry;
    ptr.write_bytes(0, cap as usize);
    ptr
}

unsafe fn slot_for(cap: i64, hash: i64, probe: i64) -> usize {
    ((hash.wrapping_add(probe)) & (cap - 1)) as usize
}

unsafe fn find_slot(
    entries: *mut TyraSetEntry,
    cap: i64,
    key: *const u8,
    eq_fn: EqFn,
    hash_fn: HashFn,
) -> (usize, bool) {
    let h = hash_fn(key);
    let mut probe: i64 = 0;
    loop {
        let idx = slot_for(cap, h, probe);
        let entry = &*entries.add(idx);
        if entry.key.is_null() {
            return (idx, false);
        }
        if eq_fn(entry.key, key) != 0 {
            return (idx, true);
        }
        probe += 1;
        if probe == cap {
            return (0, false);
        }
    }
}

unsafe fn grow_set(set: &mut TyraSet) {
    let new_cap = set.capacity * 2;
    let new_entries = alloc_entries(new_cap);
    for i in 0..set.capacity as usize {
        let old = &*set.entries.add(i);
        if old.key.is_null() {
            continue;
        }
        let h = (set.hash_fn)(old.key);
        let mut probe: i64 = 0;
        loop {
            let idx = slot_for(new_cap, h, probe);
            let slot = &mut *new_entries.add(idx);
            if slot.key.is_null() {
                slot.key = old.key;
                break;
            }
            probe += 1;
        }
    }
    set.entries = new_entries;
    set.capacity = new_cap;
}

// ── Public API ────────────────────────────────────────────────────────────────

#[unsafe(no_mangle)]
pub unsafe extern "C" fn tyra_set_new(eq_fn: EqFn, hash_fn: HashFn) -> *mut TyraSet {
    let entries = alloc_entries(INITIAL_CAPACITY);
    let set_bytes = size_of::<TyraSet>();
    let set = unsafe { GC_malloc(set_bytes) } as *mut TyraSet;
    set.write(TyraSet {
        eq_fn,
        hash_fn,
        count: 0,
        capacity: INITIAL_CAPACITY,
        entries,
    });
    set
}

/// Insert element. Idempotent: inserting an existing element is a no-op.
/// Returns the same set pointer for chaining.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tyra_set_insert(set: *mut TyraSet, key: *const u8) -> *mut TyraSet {
    if set.is_null() {
        return set;
    }
    let s = &mut *set;
    if (s.count + 1) * LOAD_FACTOR_DEN > s.capacity * LOAD_FACTOR_NUM {
        grow_set(s);
    }
    let (idx, found) = find_slot(s.entries, s.capacity, key, s.eq_fn, s.hash_fn);
    if !found {
        (*s.entries.add(idx)).key = key;
        s.count += 1;
    }
    set
}

/// Returns 1 if `key` is in the set, 0 otherwise.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tyra_set_contains(set: *const TyraSet, key: *const u8) -> c_int {
    if set.is_null() {
        return 0;
    }
    let s = &*set;
    let (_, found) = find_slot(s.entries, s.capacity, key, s.eq_fn, s.hash_fn);
    found as c_int
}

/// Returns the number of elements in the set.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tyra_set_len(set: *const TyraSet) -> i64 {
    if set.is_null() {
        return 0;
    }
    (*set).count
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

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

    #[test]
    fn empty_set_contains_false() {
        let s = unsafe { tyra_set_new(eq_i64, hash_i64) };
        assert_eq!(unsafe { tyra_set_contains(s, box_i64(1)) }, 0);
    }

    #[test]
    fn empty_set_len_zero() {
        let s = unsafe { tyra_set_new(eq_i64, hash_i64) };
        assert_eq!(unsafe { tyra_set_len(s) }, 0);
    }

    #[test]
    fn insert_then_contains() {
        let s = unsafe { tyra_set_new(eq_i64, hash_i64) };
        let s = unsafe { tyra_set_insert(s, box_i64(42)) };
        assert_eq!(unsafe { tyra_set_contains(s, box_i64(42)) }, 1);
    }

    #[test]
    fn missing_key_not_contained() {
        let s = unsafe { tyra_set_new(eq_i64, hash_i64) };
        let s = unsafe { tyra_set_insert(s, box_i64(10)) };
        assert_eq!(unsafe { tyra_set_contains(s, box_i64(99)) }, 0);
    }

    #[test]
    fn duplicate_insert_idempotent() {
        let s = unsafe { tyra_set_new(eq_i64, hash_i64) };
        let k = box_i64(5);
        let s = unsafe { tyra_set_insert(s, k) };
        let s = unsafe { tyra_set_insert(s, k) };
        assert_eq!(unsafe { tyra_set_len(s) }, 1);
    }

    #[test]
    fn len_tracks_distinct_elements() {
        let s = unsafe { tyra_set_new(eq_i64, hash_i64) };
        let s = unsafe { tyra_set_insert(s, box_i64(1)) };
        let s = unsafe { tyra_set_insert(s, box_i64(2)) };
        let s = unsafe { tyra_set_insert(s, box_i64(3)) };
        assert_eq!(unsafe { tyra_set_len(s) }, 3);
        let s = unsafe { tyra_set_insert(s, box_i64(1)) };
        assert_eq!(unsafe { tyra_set_len(s) }, 3);
    }

    #[test]
    fn grow_beyond_initial_capacity() {
        let s = unsafe { tyra_set_new(eq_i64, hash_i64) };
        let mut cur = s;
        for i in 0..32i64 {
            cur = unsafe { tyra_set_insert(cur, box_i64(i)) };
        }
        assert_eq!(unsafe { tyra_set_len(cur) }, 32);
        for i in 0..32i64 {
            assert_eq!(
                unsafe { tyra_set_contains(cur, box_i64(i)) },
                1,
                "key {i} missing"
            );
        }
        assert_eq!(unsafe { tyra_set_contains(cur, box_i64(999)) }, 0);
    }
}
