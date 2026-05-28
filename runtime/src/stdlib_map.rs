//! Generic Map<K,V> runtime — Hash Array Mapped Trie (HAMT) (ADR-0016).
//!
//! All keys and values are stored as opaque `*const u8` (i8* in LLVM) pointing
//! to GC_malloc'd boxes.  The caller (compiler-emitted code) boxes each key/
//! value before passing it to the API and unboxes the returned value pointer.
//!
//! `eq_fn` and `hash_fn` are compiler-generated per-K LLVM functions whose
//! addresses are passed at construction time (function pointers).
//!
//! All heap allocations use `GC_malloc` so Boehm GC owns every byte; no manual
//! `free` is ever needed.  Shared HAMT nodes are safe because Boehm
//! conservatively scans all pointers in GC_malloc'd memory.
//!
//! Structural immutability: `tyra_map_insert` and `tyra_map_remove` return a
//! NEW TyraMap pointer with path-copied nodes.  The original map is unchanged.
//! This eliminates the aliasing hazard that would be exposed by iteration.
//!
//! Thread-safety: not guaranteed.  Tyra tasks own their maps; no sharing.

// Internal unsafe helpers call other unsafe fns/raw-ptr ops freely; all
// safety is enforced by compiler-generated code at call sites.
#![allow(unsafe_op_in_unsafe_fn)]

use std::ffi::{CStr, c_void};
use std::os::raw::{c_char, c_int};
use std::ptr;

// ── Boehm GC extern ─────────────────────────────────────────────────────────

unsafe extern "C" {
    fn GC_malloc(size: usize) -> *mut c_void;
}

// ── Types ────────────────────────────────────────────────────────────────────

type EqFn = unsafe extern "C" fn(*const u8, *const u8) -> i32;
type HashFn = unsafe extern "C" fn(*const u8) -> i64;

/// A HAMT node. All variants are heap-allocated via GC_malloc.
#[repr(C)]
enum HamtNode {
    /// Leaf node: stores one key-value pair.
    Leaf {
        hash: u64,
        key: *const u8,
        val: *const u8,
    },
    /// Internal branch node: up to 32 children indexed by 5-bit hash slices.
    Branch {
        bitmap: u32,
        children: *mut *mut HamtNode,
    },
    /// Hash collision node: multiple entries sharing the same full 64-bit hash.
    Collision {
        hash: u64,
        count: u32,
        entries: *mut (*const u8, *const u8),
    },
}

/// Public map handle (C ABI).
#[repr(C)]
pub struct TyraMap {
    pub eq_fn: EqFn,
    pub hash_fn: HashFn,
    pub count: i64,
    // root is intentionally not pub: HamtNode is a private implementation detail.
    root: *mut HamtNode,
}

// ── HAMT constants ───────────────────────────────────────────────────────────

/// Number of bits per HAMT level (branch factor 32).
const BITS: u32 = 5;
/// Mask for extracting BITS bits.
const MASK: u64 = (1u64 << BITS) - 1;

// ── GC allocation helpers ────────────────────────────────────────────────────

unsafe fn gc_alloc<T>() -> *mut T {
    GC_malloc(size_of::<T>()) as *mut T
}

unsafe fn gc_alloc_array<T>(count: usize) -> *mut T {
    GC_malloc(size_of::<T>() * count) as *mut T
}

// ── HAMT node constructors ───────────────────────────────────────────────────

unsafe fn alloc_leaf(hash: u64, key: *const u8, val: *const u8) -> *mut HamtNode {
    let node = gc_alloc::<HamtNode>();
    node.write(HamtNode::Leaf { hash, key, val });
    node
}

unsafe fn alloc_branch(bitmap: u32, children: *mut *mut HamtNode) -> *mut HamtNode {
    let node = gc_alloc::<HamtNode>();
    node.write(HamtNode::Branch { bitmap, children });
    node
}

unsafe fn alloc_collision(
    hash: u64,
    count: u32,
    entries: *mut (*const u8, *const u8),
) -> *mut HamtNode {
    let node = gc_alloc::<HamtNode>();
    node.write(HamtNode::Collision {
        hash,
        count,
        entries,
    });
    node
}

// ── Core HAMT operations ─────────────────────────────────────────────────────

/// Look up a key in the HAMT. Returns the value pointer or null.
unsafe fn hamt_lookup(
    node: *mut HamtNode,
    hash: u64,
    key: *const u8,
    depth: u32,
    eq_fn: EqFn,
) -> *const u8 {
    if node.is_null() {
        return ptr::null();
    }
    match &*node {
        HamtNode::Leaf {
            hash: h,
            key: k,
            val: v,
        } => {
            if *h == hash && eq_fn(*k, key) != 0 {
                *v
            } else {
                ptr::null()
            }
        }
        HamtNode::Branch { bitmap, children } => {
            let idx = ((hash >> (depth * BITS)) & MASK) as u32;
            if *bitmap & (1 << idx) == 0 {
                return ptr::null();
            }
            let pos = (*bitmap & ((1 << idx) - 1)).count_ones() as usize;
            hamt_lookup(*(*children).add(pos), hash, key, depth + 1, eq_fn)
        }
        HamtNode::Collision {
            hash: h,
            count,
            entries,
        } => {
            if *h != hash {
                return ptr::null();
            }
            for i in 0..*count as usize {
                let (k, v) = *(*entries).add(i);
                if eq_fn(k, key) != 0 {
                    return v;
                }
            }
            ptr::null()
        }
    }
}

/// Insert a key-value pair. Returns a new root node (path-copy semantics).
/// `inserted` is set to true if a new key was added (as opposed to update).
unsafe fn hamt_insert(
    node: *mut HamtNode,
    hash: u64,
    key: *const u8,
    val: *const u8,
    depth: u32,
    eq_fn: EqFn,
    inserted: &mut bool,
) -> *mut HamtNode {
    if node.is_null() {
        *inserted = true;
        return alloc_leaf(hash, key, val);
    }
    match &*node {
        HamtNode::Leaf {
            hash: h,
            key: k,
            val: existing_v,
        } => {
            if *h == hash {
                if eq_fn(*k, key) != 0 {
                    // Key match: update value (count unchanged).
                    alloc_leaf(hash, key, val)
                } else {
                    // True collision: promote to Collision node.
                    *inserted = true;
                    let entries = gc_alloc_array::<(*const u8, *const u8)>(2);
                    entries.write((*k, *existing_v));
                    entries.add(1).write((key, val));
                    alloc_collision(hash, 2, entries)
                }
            } else {
                // Different hashes: create a branch holding both leaves.
                *inserted = true;
                merge_leaves(*h, node, hash, key, val, depth, eq_fn)
            }
        }
        HamtNode::Branch { bitmap, children } => {
            let idx = ((hash >> (depth * BITS)) & MASK) as u32;
            let bit = 1u32 << idx;
            let pop = (*bitmap & (bit - 1)).count_ones() as usize;
            let total = bitmap.count_ones() as usize;

            if *bitmap & bit == 0 {
                // Empty slot: insert new leaf here.
                *inserted = true;
                let new_leaf = alloc_leaf(hash, key, val);
                let new_children = gc_alloc_array::<*mut HamtNode>(total + 1);
                for i in 0..pop {
                    new_children.add(i).write(*(*children).add(i));
                }
                new_children.add(pop).write(new_leaf);
                for i in pop..total {
                    new_children.add(i + 1).write(*(*children).add(i));
                }
                alloc_branch(*bitmap | bit, new_children)
            } else {
                // Occupied slot: recurse into child.
                let child = *(*children).add(pop);
                let new_child = hamt_insert(child, hash, key, val, depth + 1, eq_fn, inserted);
                let new_children = gc_alloc_array::<*mut HamtNode>(total);
                for i in 0..total {
                    new_children.add(i).write(*(*children).add(i));
                }
                new_children.add(pop).write(new_child);
                alloc_branch(*bitmap, new_children)
            }
        }
        HamtNode::Collision {
            hash: h,
            count,
            entries,
        } => {
            if *h == hash {
                let n = *count as usize;
                for i in 0..n {
                    let (k, _v) = *(*entries).add(i);
                    if eq_fn(k, key) != 0 {
                        // Update existing entry in collision bucket.
                        let new_entries = gc_alloc_array::<(*const u8, *const u8)>(n);
                        for j in 0..n {
                            new_entries.add(j).write(*(*entries).add(j));
                        }
                        new_entries.add(i).write((key, val));
                        return alloc_collision(hash, *count, new_entries);
                    }
                }
                // New key in collision bucket.
                *inserted = true;
                let new_entries = gc_alloc_array::<(*const u8, *const u8)>(n + 1);
                for j in 0..n {
                    new_entries.add(j).write(*(*entries).add(j));
                }
                new_entries.add(n).write((key, val));
                alloc_collision(hash, *count + 1, new_entries)
            } else {
                // Different hash: branch containing this collision node + new leaf.
                *inserted = true;
                merge_collision_and_leaf(*h, node, hash, key, val, depth)
            }
        }
    }
}

/// Create a branch node containing two existing leaves with different hashes.
unsafe fn merge_leaves(
    hash_a: u64,
    leaf_a: *mut HamtNode,
    hash_b: u64,
    key_b: *const u8,
    val_b: *const u8,
    depth: u32,
    eq_fn: EqFn,
) -> *mut HamtNode {
    let idx_a = ((hash_a >> (depth * BITS)) & MASK) as u32;
    let idx_b = ((hash_b >> (depth * BITS)) & MASK) as u32;

    if idx_a == idx_b {
        // Same slot at this depth: recurse one level deeper.
        let mut inserted = false;
        let child = hamt_insert(
            leaf_a,
            hash_b,
            key_b,
            val_b,
            depth + 1,
            eq_fn,
            &mut inserted,
        );
        let children = gc_alloc_array::<*mut HamtNode>(1);
        children.write(child);
        alloc_branch(1u32 << idx_a, children)
    } else {
        let leaf_b = alloc_leaf(hash_b, key_b, val_b);
        let (first, second, bit_first, bit_second) = if idx_a < idx_b {
            (leaf_a, leaf_b, idx_a, idx_b)
        } else {
            (leaf_b, leaf_a, idx_b, idx_a)
        };
        let children = gc_alloc_array::<*mut HamtNode>(2);
        children.write(first);
        children.add(1).write(second);
        alloc_branch((1u32 << bit_first) | (1u32 << bit_second), children)
    }
}

/// Create a branch containing a collision node and a new leaf with a different hash.
unsafe fn merge_collision_and_leaf(
    hash_coll: u64,
    coll_node: *mut HamtNode,
    hash_leaf: u64,
    key_leaf: *const u8,
    val_leaf: *const u8,
    depth: u32,
) -> *mut HamtNode {
    let idx_c = ((hash_coll >> (depth * BITS)) & MASK) as u32;
    let idx_l = ((hash_leaf >> (depth * BITS)) & MASK) as u32;

    if idx_c == idx_l {
        // Same index at this depth: recurse one level deeper.
        let inner = merge_collision_and_leaf(
            hash_coll,
            coll_node,
            hash_leaf,
            key_leaf,
            val_leaf,
            depth + 1,
        );
        let children = gc_alloc_array::<*mut HamtNode>(1);
        children.write(inner);
        alloc_branch(1u32 << idx_c, children)
    } else {
        let new_leaf = alloc_leaf(hash_leaf, key_leaf, val_leaf);
        let (first, second, bit_first, bit_second) = if idx_c < idx_l {
            (coll_node, new_leaf, idx_c, idx_l)
        } else {
            (new_leaf, coll_node, idx_l, idx_c)
        };
        let children = gc_alloc_array::<*mut HamtNode>(2);
        children.write(first);
        children.add(1).write(second);
        alloc_branch((1u32 << bit_first) | (1u32 << bit_second), children)
    }
}

/// Remove a key. Returns the new root (path-copy semantics).
/// `removed` is set to true if a key was actually deleted.
unsafe fn hamt_remove(
    node: *mut HamtNode,
    hash: u64,
    key: *const u8,
    depth: u32,
    eq_fn: EqFn,
    removed: &mut bool,
) -> *mut HamtNode {
    if node.is_null() {
        return ptr::null_mut();
    }
    match &*node {
        HamtNode::Leaf {
            hash: h,
            key: k,
            val: _,
        } => {
            if *h == hash && eq_fn(*k, key) != 0 {
                *removed = true;
                ptr::null_mut()
            } else {
                node
            }
        }
        HamtNode::Branch { bitmap, children } => {
            let idx = ((hash >> (depth * BITS)) & MASK) as u32;
            let bit = 1u32 << idx;
            if *bitmap & bit == 0 {
                return node; // key not found
            }
            let pop = (*bitmap & (bit - 1)).count_ones() as usize;
            let total = bitmap.count_ones() as usize;
            let child = *(*children).add(pop);
            let new_child = hamt_remove(child, hash, key, depth + 1, eq_fn, removed);

            if !*removed {
                return node;
            }

            if new_child.is_null() {
                if total == 1 {
                    return ptr::null_mut();
                }
                let new_bitmap = *bitmap & !bit;
                let new_children = gc_alloc_array::<*mut HamtNode>(total - 1);
                for i in 0..pop {
                    new_children.add(i).write(*(*children).add(i));
                }
                for i in (pop + 1)..total {
                    new_children.add(i - 1).write(*(*children).add(i));
                }
                alloc_branch(new_bitmap, new_children)
            } else {
                let new_children = gc_alloc_array::<*mut HamtNode>(total);
                for i in 0..total {
                    new_children.add(i).write(*(*children).add(i));
                }
                new_children.add(pop).write(new_child);
                alloc_branch(*bitmap, new_children)
            }
        }
        HamtNode::Collision {
            hash: h,
            count,
            entries,
        } => {
            if *h != hash {
                return node;
            }
            let n = *count as usize;
            let mut found_idx = None;
            for i in 0..n {
                let (k, _) = *(*entries).add(i);
                if eq_fn(k, key) != 0 {
                    found_idx = Some(i);
                    break;
                }
            }
            match found_idx {
                None => node,
                Some(fi) => {
                    *removed = true;
                    if n == 1 {
                        return ptr::null_mut();
                    }
                    if n == 2 {
                        // Downgrade to a single leaf.
                        let other = if fi == 0 { 1 } else { 0 };
                        let (k2, v2) = *(*entries).add(other);
                        return alloc_leaf(hash, k2, v2);
                    }
                    let new_entries = gc_alloc_array::<(*const u8, *const u8)>(n - 1);
                    let mut j = 0usize;
                    for i in 0..n {
                        if i != fi {
                            new_entries.add(j).write(*(*entries).add(i));
                            j += 1;
                        }
                    }
                    alloc_collision(hash, *count - 1, new_entries)
                }
            }
        }
    }
}

// ── Public API ────────────────────────────────────────────────────────────────

/// Create an empty map with the given per-K eq/hash functions.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tyra_map_new(eq_fn: EqFn, hash_fn: HashFn) -> *mut TyraMap {
    let map = gc_alloc::<TyraMap>();
    map.write(TyraMap {
        eq_fn,
        hash_fn,
        count: 0,
        root: ptr::null_mut(),
    });
    map
}

/// Insert or overwrite `key → val`.  Returns a NEW TyraMap (path-copy).
/// The original map is not modified.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tyra_map_insert(
    map: *mut TyraMap,
    key: *const u8,
    val: *const u8,
) -> *mut TyraMap {
    if map.is_null() {
        return map;
    }
    let m = &*map;
    let hash = (m.hash_fn)(key) as u64;
    let mut inserted = false;
    let new_root = hamt_insert(m.root, hash, key, val, 0, m.eq_fn, &mut inserted);

    let new_map = gc_alloc::<TyraMap>();
    new_map.write(TyraMap {
        eq_fn: m.eq_fn,
        hash_fn: m.hash_fn,
        count: if inserted { m.count + 1 } else { m.count },
        root: new_root,
    });
    new_map
}

/// Remove `key` from the map.  Returns a NEW TyraMap (path-copy).
/// If the key is absent, returns a new map equal to the original.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tyra_map_remove(map: *mut TyraMap, key: *const u8) -> *mut TyraMap {
    if map.is_null() {
        return map;
    }
    let m = &*map;
    let hash = (m.hash_fn)(key) as u64;
    let mut removed = false;
    let new_root = hamt_remove(m.root, hash, key, 0, m.eq_fn, &mut removed);

    let new_map = gc_alloc::<TyraMap>();
    new_map.write(TyraMap {
        eq_fn: m.eq_fn,
        hash_fn: m.hash_fn,
        count: if removed { m.count - 1 } else { m.count },
        root: new_root,
    });
    new_map
}

/// Returns a pointer to the value box, or null if not found.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tyra_map_get(map: *const TyraMap, key: *const u8) -> *const u8 {
    if map.is_null() {
        return ptr::null();
    }
    let m = &*map;
    let hash = (m.hash_fn)(key) as u64;
    hamt_lookup(m.root, hash, key, 0, m.eq_fn)
}

/// Returns 1 if `key` is present, 0 otherwise.
/// NOTE: the compiler calls this as `tyra_map_contains`; name kept for ABI compatibility.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tyra_map_contains(map: *const TyraMap, key: *const u8) -> c_int {
    (!tyra_map_get(map, key).is_null()) as c_int
}

/// Alias for spec alignment (`tyra_map_contains_key`).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tyra_map_contains_key(map: *const TyraMap, key: *const u8) -> c_int {
    tyra_map_contains(map, key)
}

/// Returns the number of entries in the map.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tyra_map_len(map: *const TyraMap) -> i64 {
    if map.is_null() {
        return 0;
    }
    (*map).count
}

/// Traverse every entry in the map (DFS over the HAMT), calling `callback`
/// once per entry with `(ctx, key_box, val_box)`.
///
/// `ctx` is an opaque pointer forwarded unchanged to every callback
/// invocation; it is typically a pointer to a GC-managed closure env struct.
///
/// Iteration order is determined by the HAMT structure (hash order) and is
/// NOT guaranteed to be stable between runs.  Do not write tests that depend
/// on a specific visit order.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tyra_map_for_each(
    map: *const TyraMap,
    ctx: *mut c_void,
    callback: unsafe extern "C" fn(*mut c_void, *const u8, *const u8),
) {
    if map.is_null() {
        return;
    }
    hamt_for_each((*map).root, ctx, callback);
}

unsafe fn hamt_for_each(
    node: *mut HamtNode,
    ctx: *mut c_void,
    callback: unsafe extern "C" fn(*mut c_void, *const u8, *const u8),
) {
    if node.is_null() {
        return;
    }
    match &*node {
        HamtNode::Leaf { key, val, .. } => {
            callback(ctx, *key, *val);
        }
        HamtNode::Branch { bitmap, children } => {
            let count = bitmap.count_ones() as usize;
            for i in 0..count {
                hamt_for_each(*(*children).add(i), ctx, callback);
            }
        }
        HamtNode::Collision { count, entries, .. } => {
            for i in 0..*count as usize {
                let (k, v) = *(*entries).add(i);
                callback(ctx, k, v);
            }
        }
    }
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

    // ── HAMT-specific tests ──────────────────────────────────────────────────

    #[test]
    fn immutability_insert_does_not_mutate_original() {
        let m0 = unsafe { tyra_map_new(eq_i64, hash_i64) };
        let m1 = unsafe { tyra_map_insert(m0, box_i64(1), box_i64(10)) };
        let m2 = unsafe { tyra_map_insert(m1, box_i64(2), box_i64(20)) };

        // m1 must not see the key added in m2.
        assert_eq!(unsafe { tyra_map_len(m1) }, 1);
        assert!(unsafe { tyra_map_get(m1, box_i64(2)) }.is_null());

        // m2 sees both keys.
        assert_eq!(unsafe { tyra_map_len(m2) }, 2);
        assert!(!unsafe { tyra_map_get(m2, box_i64(1)) }.is_null());
        assert!(!unsafe { tyra_map_get(m2, box_i64(2)) }.is_null());
    }

    #[test]
    fn remove_existing_key() {
        let m = unsafe { tyra_map_new(eq_i64, hash_i64) };
        let m = unsafe { tyra_map_insert(m, box_i64(1), box_i64(10)) };
        let m = unsafe { tyra_map_insert(m, box_i64(2), box_i64(20)) };
        let m2 = unsafe { tyra_map_remove(m, box_i64(1)) };
        assert_eq!(unsafe { tyra_map_len(m2) }, 1);
        assert!(unsafe { tyra_map_get(m2, box_i64(1)) }.is_null());
        assert!(!unsafe { tyra_map_get(m2, box_i64(2)) }.is_null());
        // Original still intact.
        assert_eq!(unsafe { tyra_map_len(m) }, 2);
    }

    #[test]
    fn remove_missing_key_preserves_count() {
        let m = unsafe { tyra_map_new(eq_i64, hash_i64) };
        let m = unsafe { tyra_map_insert(m, box_i64(42), box_i64(1)) };
        let m2 = unsafe { tyra_map_remove(m, box_i64(99)) };
        assert_eq!(unsafe { tyra_map_len(m2) }, 1);
    }

    #[test]
    fn many_inserts_and_removes() {
        let m = unsafe { tyra_map_new(eq_i64, hash_i64) };
        let mut cur = m;
        for i in 0..100i64 {
            cur = unsafe { tyra_map_insert(cur, box_i64(i), box_i64(i * 2)) };
        }
        assert_eq!(unsafe { tyra_map_len(cur) }, 100);
        for i in (0..100i64).step_by(2) {
            cur = unsafe { tyra_map_remove(cur, box_i64(i)) };
        }
        assert_eq!(unsafe { tyra_map_len(cur) }, 50);
        for i in 0..100i64 {
            let got = unsafe { tyra_map_get(cur, box_i64(i)) };
            if i % 2 == 0 {
                assert!(got.is_null(), "key {i} should have been removed");
            } else {
                assert!(!got.is_null(), "key {i} should still be present");
                assert_eq!(unbox_i64(got), i * 2);
            }
        }
    }

    #[test]
    fn collision_bucket() {
        // Every key hashes to 42, forcing all entries into a Collision node.
        unsafe extern "C" fn hash_const(_a: *const u8) -> i64 {
            42
        }

        let m = unsafe { tyra_map_new(eq_i64, hash_const) };
        let m = unsafe { tyra_map_insert(m, box_i64(1), box_i64(10)) };
        let m = unsafe { tyra_map_insert(m, box_i64(2), box_i64(20)) };
        let m = unsafe { tyra_map_insert(m, box_i64(3), box_i64(30)) };
        assert_eq!(unsafe { tyra_map_len(m) }, 3);
        assert_eq!(unbox_i64(unsafe { tyra_map_get(m, box_i64(1)) }), 10);
        assert_eq!(unbox_i64(unsafe { tyra_map_get(m, box_i64(2)) }), 20);
        assert_eq!(unbox_i64(unsafe { tyra_map_get(m, box_i64(3)) }), 30);

        let m2 = unsafe { tyra_map_remove(m, box_i64(2)) };
        assert_eq!(unsafe { tyra_map_len(m2) }, 2);
        assert!(unsafe { tyra_map_get(m2, box_i64(2)) }.is_null());
        assert_eq!(unbox_i64(unsafe { tyra_map_get(m2, box_i64(1)) }), 10);
    }

    // ── GC / structural-sharing tests ────────────────────────────────────────
    //
    // Three tiers:
    //   1. smoke (non-ignored) — no GC_gcollect, always runs in CI.
    //      Checks shared-node correctness with N=100.
    //   2. gc_shared_nodes_survive_collection_map (#[ignore]) — N=1 000 with
    //      an explicit GC_gcollect cycle.
    //   3. gc_stress_large_n_map (#[ignore]) — N=100 000 churn with
    //      intermediate invariant checks.
    //
    // Run ignored tests single-threaded to avoid Boehm GC exclusion-range
    // conflicts between parallel test threads:
    //   cargo test -p tyra-runtime -- --ignored --test-threads=1

    // Always-on smoke: structural sharing correctness without forcing GC.
    #[test]
    fn shared_snapshot_smoke_map() {
        let n = 100i64;
        let mut cur = unsafe { tyra_map_new(eq_i64, hash_i64) };
        for i in 0..n {
            cur = unsafe { tyra_map_insert(cur, box_i64(i), box_i64(i * 3)) };
        }
        let snapshots: Vec<*mut TyraMap> = (0..10i64)
            .map(|s| unsafe { tyra_map_insert(cur, box_i64(n + s), box_i64(s * 7)) })
            .collect();

        assert_eq!(unsafe { tyra_map_len(cur) }, n);
        for i in 0..n {
            assert_eq!(
                unbox_i64(unsafe { tyra_map_get(cur, box_i64(i)) }),
                i * 3,
                "base: key {i} wrong"
            );
        }
        for (s, &snap) in snapshots.iter().enumerate() {
            let s = s as i64;
            assert_eq!(
                unsafe { tyra_map_len(snap) },
                n + 1,
                "snapshot {s}: wrong len"
            );
            assert_eq!(
                unbox_i64(unsafe { tyra_map_get(snap, box_i64(n + s)) }),
                s * 7,
                "snapshot {s}: private value wrong"
            );
            assert!(
                !unsafe { tyra_map_get(snap, box_i64(0)) }.is_null(),
                "snapshot {s}: shared key 0 missing"
            );
        }
    }

    // GC cycle: shared nodes must survive an explicit GC_gcollect.
    #[test]
    #[ignore]
    fn gc_shared_nodes_survive_collection_map() {
        unsafe extern "C" {
            fn GC_gcollect();
        }

        let n = 1_000i64;
        let mut cur = unsafe { tyra_map_new(eq_i64, hash_i64) };
        for i in 0..n {
            cur = unsafe { tyra_map_insert(cur, box_i64(i), box_i64(i * 3)) };
        }
        let snapshots: Vec<*mut TyraMap> = (0..50i64)
            .map(|s| unsafe { tyra_map_insert(cur, box_i64(n + s), box_i64(s * 7)) })
            .collect();

        unsafe {
            GC_gcollect();
        }

        assert_eq!(unsafe { tyra_map_len(cur) }, n, "base: wrong len after GC");
        for i in 0..n {
            let got = unsafe { tyra_map_get(cur, box_i64(i)) };
            assert!(!got.is_null(), "base: key {i} missing after GC");
            assert_eq!(
                unbox_i64(got),
                i * 3,
                "base: wrong value for key {i} after GC"
            );
        }
        for (s, &snap) in snapshots.iter().enumerate() {
            let s = s as i64;
            assert_eq!(
                unsafe { tyra_map_len(snap) },
                n + 1,
                "snapshot {s}: wrong len after GC"
            );
            let got = unsafe { tyra_map_get(snap, box_i64(n + s)) };
            assert!(!got.is_null(), "snapshot {s}: private key missing after GC");
            assert_eq!(
                unbox_i64(got),
                s * 7,
                "snapshot {s}: wrong private value after GC"
            );
            assert!(
                !unsafe { tyra_map_get(snap, box_i64(0)) }.is_null(),
                "snapshot {s}: shared key 0 missing after GC"
            );
        }
    }

    // Long-running churn with intermediate invariant checks after each GC.
    #[test]
    #[ignore]
    fn gc_stress_large_n_map() {
        unsafe extern "C" {
            fn GC_gcollect();
        }

        let n = 100_000i64;
        let mut cur = unsafe { tyra_map_new(eq_i64, hash_i64) };

        // Phase 1: insert N entries, checking invariants every 10 K.
        for i in 0..n {
            cur = unsafe { tyra_map_insert(cur, box_i64(i), box_i64(i)) };
            if i % 10_000 == 9_999 {
                unsafe {
                    GC_gcollect();
                }
                let expected_len = i + 1;
                assert_eq!(
                    unsafe { tyra_map_len(cur) },
                    expected_len,
                    "phase 1: len at i={i}"
                );
                // Representative key present.
                assert!(
                    !unsafe { tyra_map_get(cur, box_i64(0)) }.is_null(),
                    "phase 1: key 0 missing at i={i}"
                );
                // Key not yet inserted absent.
                assert!(
                    unsafe { tyra_map_get(cur, box_i64(i + 1)) }.is_null(),
                    "phase 1: future key present at i={i}"
                );
            }
        }
        assert_eq!(unsafe { tyra_map_len(cur) }, n);

        // Phase 2: remove every even key, checking invariants every 10 K.
        for i in (0..n).step_by(2) {
            cur = unsafe { tyra_map_remove(cur, box_i64(i)) };
            if i % 10_000 == 9_998 {
                unsafe {
                    GC_gcollect();
                }
                let removed_so_far = (i / 2) + 1;
                assert_eq!(
                    unsafe { tyra_map_len(cur) },
                    n - removed_so_far,
                    "phase 2: len after removing up to key {i}"
                );
                // The just-removed key must be absent.
                assert!(
                    unsafe { tyra_map_get(cur, box_i64(i)) }.is_null(),
                    "phase 2: removed key {i} still present"
                );
                // The preceding odd key must still be present.
                if i > 0 {
                    assert!(
                        !unsafe { tyra_map_get(cur, box_i64(i - 1)) }.is_null(),
                        "phase 2: retained odd key {} missing",
                        i - 1
                    );
                }
            }
        }
        assert_eq!(unsafe { tyra_map_len(cur) }, n / 2);

        // Phase 3: insert another N entries, checking invariants every 10 K.
        for i in n..(2 * n) {
            cur = unsafe { tyra_map_insert(cur, box_i64(i), box_i64(i)) };
            if i % 10_000 == 9_999 {
                unsafe {
                    GC_gcollect();
                }
                let inserted_p3 = i - n + 1;
                assert_eq!(
                    unsafe { tyra_map_len(cur) },
                    n / 2 + inserted_p3,
                    "phase 3: len at i={i}"
                );
                assert!(
                    !unsafe { tyra_map_get(cur, box_i64(i)) }.is_null(),
                    "phase 3: just-inserted key {i} missing"
                );
                // An odd phase-1 key must still be present.
                assert!(
                    !unsafe { tyra_map_get(cur, box_i64(1)) }.is_null(),
                    "phase 3: phase-1 odd key 1 missing at i={i}"
                );
            }
        }
        assert_eq!(unsafe { tyra_map_len(cur) }, n / 2 + n);

        // Final GC + full integrity sweep.
        unsafe {
            GC_gcollect();
        }
        for i in (1..n).step_by(2) {
            assert!(
                !unsafe { tyra_map_get(cur, box_i64(i)) }.is_null(),
                "final: phase-1 odd key {i} missing"
            );
        }
        for i in n..(2 * n) {
            assert!(
                !unsafe { tyra_map_get(cur, box_i64(i)) }.is_null(),
                "final: phase-3 key {i} missing"
            );
        }
    }
}
