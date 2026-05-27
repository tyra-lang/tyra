//! Generic Map<K,V> runtime — open-addressing hash table (ADR-0015).
//!
//! All keys and values are stored as opaque `*const u8` (i8* in LLVM) pointing
//! to GC_malloc'd boxes.  The caller (compiler-emitted code) boxes each key/
//! value before passing it to the API and unboxes the returned value pointer.
//!
//! `eq_fn` and `hash_fn` are compiler-generated per-K LLVM functions whose
//! addresses are passed at construction time (function pointers).
//!
//! All heap allocations use `GC_malloc` so Boehm GC owns every byte; no manual
//! `free` is ever needed.  The conservative GC sees the entries array (a
//! GC_malloc'd pointer) and retains every non-null key/value pointer through it.
//!
//! Thread-safety: not guaranteed.  Tyra tasks own their maps; no sharing.

// Internal unsafe helpers call other unsafe fns/raw-ptr ops freely; all
// safety is enforced by compiler-generated code at call sites.
#![allow(unsafe_op_in_unsafe_fn)]

use std::ffi::{CStr, c_void};
use std::os::raw::{c_char, c_int};

// ── Boehm GC extern ─────────────────────────────────────────────────────────

unsafe extern "C" {
    fn GC_malloc(size: usize) -> *mut c_void;
}

// ── Types ────────────────────────────────────────────────────────────────────

type EqFn = unsafe extern "C" fn(*const u8, *const u8) -> i32;
type HashFn = unsafe extern "C" fn(*const u8) -> i64;

#[repr(C)]
struct TyraMapEntry {
    key: *const u8, // null → empty slot
    val: *const u8,
}

#[repr(C)]
pub struct TyraMap {
    eq_fn: EqFn,
    hash_fn: HashFn,
    count: i64,
    capacity: i64, // always a power of 2
    entries: *mut TyraMapEntry,
}

// ── Internal helpers ─────────────────────────────────────────────────────────

const INITIAL_CAPACITY: i64 = 8;
const LOAD_FACTOR_NUM: i64 = 3; // 3/4 = 0.75
const LOAD_FACTOR_DEN: i64 = 4;

unsafe fn alloc_entries(cap: i64) -> *mut TyraMapEntry {
    let bytes = (cap as usize) * size_of::<TyraMapEntry>();
    let ptr = unsafe { GC_malloc(bytes) } as *mut TyraMapEntry;
    ptr.write_bytes(0, cap as usize);
    ptr
}

unsafe fn slot_for(_entries: *mut TyraMapEntry, cap: i64, hash: i64, probe: i64) -> usize {
    ((hash.wrapping_add(probe)) & (cap - 1)) as usize
}

unsafe fn find_slot(
    entries: *mut TyraMapEntry,
    cap: i64,
    key: *const u8,
    eq_fn: EqFn,
    hash_fn: HashFn,
) -> (usize, bool) {
    let h = hash_fn(key);
    let mut probe: i64 = 0;
    loop {
        let idx = slot_for(entries, cap, h, probe);
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

unsafe fn grow_map(map: &mut TyraMap) {
    let new_cap = map.capacity * 2;
    let new_entries = alloc_entries(new_cap);
    for i in 0..map.capacity as usize {
        let old = &*map.entries.add(i);
        if old.key.is_null() {
            continue;
        }
        let h = (map.hash_fn)(old.key);
        let mut probe: i64 = 0;
        loop {
            let idx = slot_for(new_entries, new_cap, h, probe);
            let slot = &mut *new_entries.add(idx);
            if slot.key.is_null() {
                slot.key = old.key;
                slot.val = old.val;
                break;
            }
            probe += 1;
        }
    }
    map.entries = new_entries;
    map.capacity = new_cap;
}

// ── Public API ────────────────────────────────────────────────────────────────

/// Create an empty map with the given per-K eq/hash functions.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tyra_map_new(eq_fn: EqFn, hash_fn: HashFn) -> *mut TyraMap {
    let entries = alloc_entries(INITIAL_CAPACITY);
    let map_bytes = size_of::<TyraMap>();
    let map = unsafe { GC_malloc(map_bytes) } as *mut TyraMap;
    map.write(TyraMap {
        eq_fn,
        hash_fn,
        count: 0,
        capacity: INITIAL_CAPACITY,
        entries,
    });
    map
}

/// Insert or overwrite `key → val`.  Returns the same map pointer.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tyra_map_insert(
    map: *mut TyraMap,
    key: *const u8,
    val: *const u8,
) -> *mut TyraMap {
    if map.is_null() {
        return map;
    }
    let m = &mut *map;
    if (m.count + 1) * LOAD_FACTOR_DEN > m.capacity * LOAD_FACTOR_NUM {
        grow_map(m);
    }
    let (idx, found) = find_slot(m.entries, m.capacity, key, m.eq_fn, m.hash_fn);
    let slot = &mut *m.entries.add(idx);
    slot.key = key;
    slot.val = val;
    if !found {
        m.count += 1;
    }
    map
}

/// Returns a pointer to the value box, or null if not found.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tyra_map_get(map: *const TyraMap, key: *const u8) -> *const u8 {
    if map.is_null() {
        return std::ptr::null();
    }
    let m = &*map;
    let (idx, found) = find_slot(m.entries, m.capacity, key, m.eq_fn, m.hash_fn);
    if found {
        (*m.entries.add(idx)).val
    } else {
        std::ptr::null()
    }
}

/// Returns 1 if `key` is present, 0 otherwise.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tyra_map_contains(map: *const TyraMap, key: *const u8) -> c_int {
    if map.is_null() {
        return 0;
    }
    let m = &*map;
    let (_, found) = find_slot(m.entries, m.capacity, key, m.eq_fn, m.hash_fn);
    found as c_int
}

/// Returns the number of entries in the map.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tyra_map_len(map: *const TyraMap) -> i64 {
    if map.is_null() {
        return 0;
    }
    (*map).count
}

// ── Hashing/equality helpers for compiler-emitted tyra_eq/hash_* functions ───

/// FNV-1a hash of a null-terminated C string.
#[unsafe(no_mangle)]
#[allow(clippy::not_unsafe_ptr_arg_deref)]
pub extern "C" fn tyra_hash_cstr(s: *const c_char) -> i64 {
    if s.is_null() {
        return 0;
    }
    let bytes = unsafe { CStr::from_ptr(s) }.to_bytes();
    let mut hash: u64 = 14695981039346656037;
    for &b in bytes {
        hash ^= b as u64;
        hash = hash.wrapping_mul(1099511628211);
    }
    hash as i64
}

/// Equality for null-terminated C strings.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tyra_cstr_eq(a: *const c_char, b: *const c_char) -> c_int {
    if a.is_null() && b.is_null() {
        return 1;
    }
    if a.is_null() || b.is_null() {
        return 0;
    }
    (unsafe { CStr::from_ptr(a) } == unsafe { CStr::from_ptr(b) }) as c_int
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

    fn unbox_i64(p: *const u8) -> i64 {
        unsafe { *(p as *const i64) }
    }

    #[test]
    fn empty_map_get_returns_null() {
        let m = unsafe { tyra_map_new(eq_i64, hash_i64) };
        let k = box_i64(1);
        assert!(unsafe { tyra_map_get(m, k) }.is_null());
    }

    #[test]
    fn empty_map_contains_false() {
        let m = unsafe { tyra_map_new(eq_i64, hash_i64) };
        assert_eq!(unsafe { tyra_map_contains(m, box_i64(1)) }, 0);
    }

    #[test]
    fn insert_then_get() {
        let m = unsafe { tyra_map_new(eq_i64, hash_i64) };
        let m = unsafe { tyra_map_insert(m, box_i64(10), box_i64(100)) };
        let got = unsafe { tyra_map_get(m, box_i64(10)) };
        assert!(!got.is_null());
        assert_eq!(unbox_i64(got), 100);
    }

    #[test]
    fn missing_key_null() {
        let m = unsafe { tyra_map_new(eq_i64, hash_i64) };
        let m = unsafe { tyra_map_insert(m, box_i64(10), box_i64(100)) };
        assert!(unsafe { tyra_map_get(m, box_i64(99)) }.is_null());
    }

    #[test]
    fn overwrite_does_not_grow_count() {
        let m = unsafe { tyra_map_new(eq_i64, hash_i64) };
        let k = box_i64(5);
        let m = unsafe { tyra_map_insert(m, k, box_i64(10)) };
        let m = unsafe { tyra_map_insert(m, k, box_i64(20)) };
        assert_eq!(unsafe { tyra_map_len(m) }, 1);
        assert_eq!(unbox_i64(unsafe { tyra_map_get(m, k) }), 20);
    }

    #[test]
    fn len_tracks_distinct_keys() {
        let m = unsafe { tyra_map_new(eq_i64, hash_i64) };
        let m = unsafe { tyra_map_insert(m, box_i64(1), box_i64(0)) };
        let m = unsafe { tyra_map_insert(m, box_i64(2), box_i64(0)) };
        let m = unsafe { tyra_map_insert(m, box_i64(3), box_i64(0)) };
        assert_eq!(unsafe { tyra_map_len(m) }, 3);
        let _ = unsafe { tyra_map_insert(m, box_i64(1), box_i64(99)) };
        assert_eq!(unsafe { tyra_map_len(m) }, 3);
    }

    #[test]
    fn grow_beyond_initial_capacity() {
        let m = unsafe { tyra_map_new(eq_i64, hash_i64) };
        let mut cur = m;
        for i in 0..32i64 {
            cur = unsafe { tyra_map_insert(cur, box_i64(i), box_i64(i * 10)) };
        }
        assert_eq!(unsafe { tyra_map_len(cur) }, 32);
        for i in 0..32i64 {
            let got = unsafe { tyra_map_get(cur, box_i64(i)) };
            assert!(!got.is_null(), "key {i} missing after grow");
            assert_eq!(unbox_i64(got), i * 10);
        }
    }

    #[test]
    fn hash_cstr_stable() {
        use std::ffi::CString;
        let s = CString::new("hello").unwrap();
        let h1 = tyra_hash_cstr(s.as_ptr());
        let h2 = tyra_hash_cstr(s.as_ptr());
        assert_eq!(h1, h2);
        assert_ne!(h1, 0);
    }

    #[test]
    fn cstr_eq_same() {
        use std::ffi::CString;
        let a = CString::new("abc").unwrap();
        let b = CString::new("abc").unwrap();
        assert_eq!(unsafe { tyra_cstr_eq(a.as_ptr(), b.as_ptr()) }, 1);
    }

    #[test]
    fn cstr_eq_different() {
        use std::ffi::CString;
        let a = CString::new("abc").unwrap();
        let b = CString::new("xyz").unwrap();
        assert_eq!(unsafe { tyra_cstr_eq(a.as_ptr(), b.as_ptr()) }, 0);
    }
}
