//! Generic Set<T> runtime — Hash Array Mapped Trie (HAMT) (ADR-0016).
//!
//! Elements are stored as opaque `*const u8` pointing to GC_malloc'd boxes.
//! `eq_fn` and `hash_fn` are compiler-generated per-T LLVM functions passed
//! at construction time. All allocations use Boehm GC; no manual free needed.
//!
//! Structural immutability: `tyra_set_insert` and `tyra_set_remove` return a
//! NEW TyraSet pointer (path-copy). The original set is unchanged.

#![allow(unsafe_op_in_unsafe_fn)]

use std::ffi::c_void;
use std::os::raw::c_int;
use std::ptr;

unsafe extern "C" {
    fn GC_malloc(size: usize) -> *mut c_void;
}

type EqFn = unsafe extern "C" fn(*const u8, *const u8) -> i32;
type HashFn = unsafe extern "C" fn(*const u8) -> i64;

/// A HAMT node for Set (keys only, no values).
#[repr(C)]
enum HamtNode {
    Leaf {
        hash: u64,
        key: *const u8,
    },
    Branch {
        bitmap: u32,
        children: *mut *mut HamtNode,
    },
    Collision {
        hash: u64,
        count: u32,
        keys: *mut *const u8,
    },
}

#[repr(C)]
pub struct TyraSet {
    pub eq_fn: EqFn,
    pub hash_fn: HashFn,
    pub count: i64,
    // root is intentionally not pub: HamtNode is a private implementation detail.
    root: *mut HamtNode,
}

const BITS: u32 = 5;
const MASK: u64 = (1u64 << BITS) - 1;

unsafe fn gc_alloc<T>() -> *mut T {
    GC_malloc(size_of::<T>()) as *mut T
}

unsafe fn gc_alloc_array<T>(count: usize) -> *mut T {
    GC_malloc(size_of::<T>() * count) as *mut T
}

unsafe fn alloc_leaf(hash: u64, key: *const u8) -> *mut HamtNode {
    let node = gc_alloc::<HamtNode>();
    node.write(HamtNode::Leaf { hash, key });
    node
}

unsafe fn alloc_branch(bitmap: u32, children: *mut *mut HamtNode) -> *mut HamtNode {
    let node = gc_alloc::<HamtNode>();
    node.write(HamtNode::Branch { bitmap, children });
    node
}

unsafe fn alloc_collision(hash: u64, count: u32, keys: *mut *const u8) -> *mut HamtNode {
    let node = gc_alloc::<HamtNode>();
    node.write(HamtNode::Collision { hash, count, keys });
    node
}

unsafe fn hamt_contains(node: *mut HamtNode, hash: u64, key: *const u8, depth: u32, eq_fn: EqFn) -> bool {
    if node.is_null() {
        return false;
    }
    match &*node {
        HamtNode::Leaf { hash: h, key: k } => *h == hash && eq_fn(*k, key) != 0,
        HamtNode::Branch { bitmap, children } => {
            let idx = ((hash >> (depth * BITS)) & MASK) as u32;
            if *bitmap & (1 << idx) == 0 {
                return false;
            }
            let pos = (*bitmap & ((1 << idx) - 1)).count_ones() as usize;
            hamt_contains(*(*children).add(pos), hash, key, depth + 1, eq_fn)
        }
        HamtNode::Collision { hash: h, count, keys } => {
            if *h != hash {
                return false;
            }
            for i in 0..*count as usize {
                if eq_fn(*(*keys).add(i), key) != 0 {
                    return true;
                }
            }
            false
        }
    }
}

unsafe fn hamt_insert(
    node: *mut HamtNode,
    hash: u64,
    key: *const u8,
    depth: u32,
    eq_fn: EqFn,
    inserted: &mut bool,
) -> *mut HamtNode {
    if node.is_null() {
        *inserted = true;
        return alloc_leaf(hash, key);
    }
    match &*node {
        HamtNode::Leaf { hash: h, key: k } => {
            if *h == hash {
                if eq_fn(*k, key) != 0 {
                    // Already present: no-op (return same shape, count unchanged).
                    node
                } else {
                    // True collision: promote to Collision node.
                    *inserted = true;
                    let keys = gc_alloc_array::<*const u8>(2);
                    keys.write(*k);
                    keys.add(1).write(key);
                    alloc_collision(hash, 2, keys)
                }
            } else {
                *inserted = true;
                merge_leaves(*h, node, hash, key, depth, eq_fn)
            }
        }
        HamtNode::Branch { bitmap, children } => {
            let idx = ((hash >> (depth * BITS)) & MASK) as u32;
            let bit = 1u32 << idx;
            let pop = (*bitmap & (bit - 1)).count_ones() as usize;
            let total = bitmap.count_ones() as usize;

            if *bitmap & bit == 0 {
                *inserted = true;
                let new_leaf = alloc_leaf(hash, key);
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
                let child = *(*children).add(pop);
                let new_child = hamt_insert(child, hash, key, depth + 1, eq_fn, inserted);
                let new_children = gc_alloc_array::<*mut HamtNode>(total);
                for i in 0..total {
                    new_children.add(i).write(*(*children).add(i));
                }
                new_children.add(pop).write(new_child);
                alloc_branch(*bitmap, new_children)
            }
        }
        HamtNode::Collision { hash: h, count, keys } => {
            if *h == hash {
                let n = *count as usize;
                for i in 0..n {
                    if eq_fn(*(*keys).add(i), key) != 0 {
                        return node; // already present
                    }
                }
                *inserted = true;
                let new_keys = gc_alloc_array::<*const u8>(n + 1);
                for j in 0..n {
                    new_keys.add(j).write(*(*keys).add(j));
                }
                new_keys.add(n).write(key);
                alloc_collision(hash, *count + 1, new_keys)
            } else {
                *inserted = true;
                merge_collision_and_leaf(*h, node, hash, key, depth)
            }
        }
    }
}

unsafe fn merge_leaves(
    hash_a: u64,
    leaf_a: *mut HamtNode,
    hash_b: u64,
    key_b: *const u8,
    depth: u32,
    eq_fn: EqFn,
) -> *mut HamtNode {
    let idx_a = ((hash_a >> (depth * BITS)) & MASK) as u32;
    let idx_b = ((hash_b >> (depth * BITS)) & MASK) as u32;

    if idx_a == idx_b {
        let mut inserted = false;
        let child = hamt_insert(leaf_a, hash_b, key_b, depth + 1, eq_fn, &mut inserted);
        let children = gc_alloc_array::<*mut HamtNode>(1);
        children.write(child);
        alloc_branch(1u32 << idx_a, children)
    } else {
        let leaf_b = alloc_leaf(hash_b, key_b);
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

unsafe fn merge_collision_and_leaf(
    hash_coll: u64,
    coll_node: *mut HamtNode,
    hash_leaf: u64,
    key_leaf: *const u8,
    depth: u32,
) -> *mut HamtNode {
    let idx_c = ((hash_coll >> (depth * BITS)) & MASK) as u32;
    let idx_l = ((hash_leaf >> (depth * BITS)) & MASK) as u32;

    if idx_c == idx_l {
        let inner = merge_collision_and_leaf(hash_coll, coll_node, hash_leaf, key_leaf, depth + 1);
        let children = gc_alloc_array::<*mut HamtNode>(1);
        children.write(inner);
        alloc_branch(1u32 << idx_c, children)
    } else {
        let new_leaf = alloc_leaf(hash_leaf, key_leaf);
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
        HamtNode::Leaf { hash: h, key: k } => {
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
                return node;
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
        HamtNode::Collision { hash: h, count, keys } => {
            if *h != hash {
                return node;
            }
            let n = *count as usize;
            let mut found_idx = None;
            for i in 0..n {
                if eq_fn(*(*keys).add(i), key) != 0 {
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
                        let other = if fi == 0 { 1 } else { 0 };
                        let k2 = *(*keys).add(other);
                        return alloc_leaf(hash, k2);
                    }
                    let new_keys = gc_alloc_array::<*const u8>(n - 1);
                    let mut j = 0usize;
                    for i in 0..n {
                        if i != fi {
                            new_keys.add(j).write(*(*keys).add(i));
                            j += 1;
                        }
                    }
                    alloc_collision(hash, *count - 1, new_keys)
                }
            }
        }
    }
}

// ── Public API ────────────────────────────────────────────────────────────────

#[unsafe(no_mangle)]
pub unsafe extern "C" fn tyra_set_new(eq_fn: EqFn, hash_fn: HashFn) -> *mut TyraSet {
    let set = gc_alloc::<TyraSet>();
    set.write(TyraSet {
        eq_fn,
        hash_fn,
        count: 0,
        root: ptr::null_mut(),
    });
    set
}

/// Insert element. Idempotent: inserting an existing element is a no-op.
/// Returns a NEW TyraSet (path-copy semantics).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tyra_set_insert(set: *mut TyraSet, key: *const u8) -> *mut TyraSet {
    if set.is_null() {
        return set;
    }
    let s = &*set;
    let hash = (s.hash_fn)(key) as u64;
    let mut inserted = false;
    let new_root = hamt_insert(s.root, hash, key, 0, s.eq_fn, &mut inserted);

    let new_set = gc_alloc::<TyraSet>();
    new_set.write(TyraSet {
        eq_fn: s.eq_fn,
        hash_fn: s.hash_fn,
        count: if inserted { s.count + 1 } else { s.count },
        root: new_root,
    });
    new_set
}

/// Remove element. Returns a NEW TyraSet (path-copy semantics).
/// If the element is absent, returns a new set equal to the original.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tyra_set_remove(set: *mut TyraSet, key: *const u8) -> *mut TyraSet {
    if set.is_null() {
        return set;
    }
    let s = &*set;
    let hash = (s.hash_fn)(key) as u64;
    let mut removed = false;
    let new_root = hamt_remove(s.root, hash, key, 0, s.eq_fn, &mut removed);

    let new_set = gc_alloc::<TyraSet>();
    new_set.write(TyraSet {
        eq_fn: s.eq_fn,
        hash_fn: s.hash_fn,
        count: if removed { s.count - 1 } else { s.count },
        root: new_root,
    });
    new_set
}

/// Returns 1 if `key` is in the set, 0 otherwise.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tyra_set_contains(set: *const TyraSet, key: *const u8) -> c_int {
    if set.is_null() {
        return 0;
    }
    let s = &*set;
    let hash = (s.hash_fn)(key) as u64;
    hamt_contains(s.root, hash, key, 0, s.eq_fn) as c_int
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

    // ── HAMT-specific tests ──────────────────────────────────────────────────

    #[test]
    fn immutability_insert_does_not_mutate_original() {
        let s0 = unsafe { tyra_set_new(eq_i64, hash_i64) };
        let s1 = unsafe { tyra_set_insert(s0, box_i64(1)) };
        let s2 = unsafe { tyra_set_insert(s1, box_i64(2)) };

        // s1 must not see element added to s2.
        assert_eq!(unsafe { tyra_set_len(s1) }, 1);
        assert_eq!(unsafe { tyra_set_contains(s1, box_i64(2)) }, 0);

        // s2 sees both elements.
        assert_eq!(unsafe { tyra_set_len(s2) }, 2);
        assert_eq!(unsafe { tyra_set_contains(s2, box_i64(1)) }, 1);
        assert_eq!(unsafe { tyra_set_contains(s2, box_i64(2)) }, 1);
    }

    #[test]
    fn remove_existing_element() {
        let s = unsafe { tyra_set_new(eq_i64, hash_i64) };
        let s = unsafe { tyra_set_insert(s, box_i64(1)) };
        let s = unsafe { tyra_set_insert(s, box_i64(2)) };
        let s2 = unsafe { tyra_set_remove(s, box_i64(1)) };
        assert_eq!(unsafe { tyra_set_len(s2) }, 1);
        assert_eq!(unsafe { tyra_set_contains(s2, box_i64(1)) }, 0);
        assert_eq!(unsafe { tyra_set_contains(s2, box_i64(2)) }, 1);
        // Original still intact.
        assert_eq!(unsafe { tyra_set_len(s) }, 2);
    }

    #[test]
    fn remove_missing_element_preserves_count() {
        let s = unsafe { tyra_set_new(eq_i64, hash_i64) };
        let s = unsafe { tyra_set_insert(s, box_i64(42)) };
        let s2 = unsafe { tyra_set_remove(s, box_i64(99)) };
        assert_eq!(unsafe { tyra_set_len(s2) }, 1);
    }

    #[test]
    fn collision_bucket() {
        unsafe extern "C" fn hash_const(_a: *const u8) -> i64 { 7 }

        let s = unsafe { tyra_set_new(eq_i64, hash_const) };
        let s = unsafe { tyra_set_insert(s, box_i64(1)) };
        let s = unsafe { tyra_set_insert(s, box_i64(2)) };
        let s = unsafe { tyra_set_insert(s, box_i64(3)) };
        assert_eq!(unsafe { tyra_set_len(s) }, 3);

        let s2 = unsafe { tyra_set_remove(s, box_i64(2)) };
        assert_eq!(unsafe { tyra_set_len(s2) }, 2);
        assert_eq!(unsafe { tyra_set_contains(s2, box_i64(2)) }, 0);
        assert_eq!(unsafe { tyra_set_contains(s2, box_i64(1)) }, 1);
    }
}
