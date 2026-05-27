# ADR 0016: Persistent collections — HAMT-based `Map<K,V>` and `Set<T>`

- **Status**: Accepted
- **Date**: 2026-05-27
- **Spec sections affected**: §17.3.6 (Map), §17.3.7 (Set)

## Context

v0.6.0 shipped `Map<K,V>` and `Set<T>` with an open-addressing in-place hash table implementation.
The compiler specification (§17.3.6-7) states "言語の観点では非破壊的" (non-destructive from the language perspective),
and ADR 0001 establishes that collections use **data (reference) semantics**.

However, the v0.6.0 **runtime implementation mutates in-place**: when a binding `m` is aliased as `m2`,
and then `m.insert(k, v)` is called, the runtime modifies the table cells in-memory, and **both `m` and `m2` observe the update**.
This is tolerated in v0.6.0 because iteration is not yet implemented.

**The aliasing hazard becomes critical in v0.7.0 when iteration is added.**

v0.7.0 adds `for k, v in m { ... }` and `for v in s { ... }` (ADR 0012/0013 iteration semantics).
If iteration uses open-addressing in-place table traversal, then:

```
m := {1: "a"}
m2 := m                          // m2 aliases the same table object
for k, v in m2 { ... }           // reading table state
  m.insert(2, "b")              // mutates table in-place
                                // m2's iteration observes the update → undefined behavior
```

**This violates the spec contract** ("non-destructive from language perspective") and violates
AGENTS.md's design principle "Explicitness over implicitness / less power, more determinism."

The spec defines observable semantics—whether iteration sees updates to an alias should be **explicit and predictable**,
not an accident of runtime mutation.

## Decision

**Reimplement `Map<K,V>` and `Set<T>` as HAMT** (Hash Array Mapped Trie) — a **persistent data structure**
with structural sharing and **true copy-on-write semantics via path-copying**.

### 1. HAMT structure and update semantics

**Path-copying principle**: `m.insert(k, v)` and `m.remove(k)` return a **new `Map<K,V>`** (immutable binding).
The original binding `m` retains its previous state.

At runtime, this is implemented via **structural sharing**:
- Only nodes along the path from root to the inserted/removed key are **copied**.
- All sibling subtrees are **shared** between the old Map and the new Map.
- Example: inserting into a Map with 1000 entries copies ~7–10 nodes (tree depth ≈ log₃₂ n),
  leaving 990+ nodes shared.

```
// v0.6.0 (in-place): m.insert modifies m in-place
let m = {1: "a"}
let m2 = m                       // m2 aliases the table
m = m.insert(2, "b")            // in-place mutation; m2 now sees {1: "a", 2: "b"} too
                                // aliasing hazard

// v0.7.0+ (persistent HAMT): m.insert returns new Map
let m = {1: "a"}
let m2 = m                       // m2 is a reference to the same root node
m = m.insert(2, "b")            // returns new Map with new root; m2 unchanged
                                // m := {1: "a", 2: "b"}
                                // m2 := {1: "a"}
```

### 2. HAMT design parameters

- **Branch factor**: 32 (each node has up to 32 children)
- **Bits per level**: 5 (32 = 2^5)
- **Max depth**: 13 (64-bit hash space; 64 / 5 = 12.8 levels)
- **Node types**:
  - **Bitmap node**: `u32 bitmap` + variable-length `children[]` array (size = popcount(bitmap))
    - Bitmap tracks which of 32 slots contain children
    - Only non-null children are stored (sparse representation)
  - **Leaf node**: `i8* hash` + `i8* key` + `i8* val` (single key-value entry)
  - **Collision node**: handles multiple entries with the same hash (rare; multiple keys mapping to same hash prefix)

**Path-copy algorithm for insert**:
1. Hash the key: `h := hash_fn(key)` → 64-bit value
2. Traverse from root, using 5-bit chunks of `h` to select child index at each level
3. On traversal, **copy each node** before modifying its children array
4. At leaf, create new leaf (if inserting new key) or return new leaf (if updating)
5. Backtrack, linking copied nodes with updated child pointers
6. Return new root

**Siblings are not copied**; they remain referenced by both old and new root.

### 3. API surface (confirmed, matching v0.6.0 semantics but with persistence)

```tyra
// Map<K,V> methods
fn insert(m: Map<K,V>, k: K, v: V) -> Map<K,V>
  // Returns new Map with (k,v) inserted
  // If k already exists, value is updated
  // Original m unchanged

fn remove(m: Map<K,V>, k: K) -> Map<K,V>
  // Returns new Map with key k removed
  // If k not present, returns Map unchanged
  // Original m unchanged

fn contains(m: Map<K,V>, k: K) -> Bool
  // True if key k exists (unchanged from v0.6.0)

fn len(m: Map<K,V>) -> Int
  // Number of entries (unchanged from v0.6.0)

// Set<T> methods
fn insert(s: Set<T>, v: T) -> Set<T>
  // Returns new Set with v inserted
  // If v already exists, returns Set unchanged (idempotent)
  // Original s unchanged

fn remove(s: Set<T>, v: T) -> Set<T>
  // Returns new Set with v removed
  // If v not present, returns Set unchanged
  // Original s unchanged

fn contains(s: Set<T>, v: T) -> Bool
  // True if v exists (unchanged from v0.6.0)

fn len(s: Set<T>) -> Int
  // Number of elements (unchanged from v0.6.0)
```

**Iteration** (new in v0.7.0):
```tyra
for k, v in m { ... }    // iterates key-value pairs
for v in s { ... }       // iterates elements
```

Iteration is implemented via **DFS (depth-first search) callback traversal** of the HAMT tree.
Order: **HAMT DFS traversal order = hash-determined, not insertion order**.

### 4. Iteration semantics and safety

**Order guarantee**: Iteration order is **deterministic for a fixed set of keys** but **not insertion order**.
Two Maps with the same (k,v) pairs will iterate in the same order. Two Maps built via different insertion sequences
may iterate in different orders if hash function differs (but standard hash_fn is deterministic).

**Mutation during iteration**: Safe due to persistence.
```tyra
let m = {1: "a", 2: "b"}
for k, v in m {
  if k == 1 {
    m = m.insert(3, "c")  // m is rebound to new Map
  }
  // The iteration continues over the original m (from before insert)
  // It does NOT see the new key 3
}
```

The iterator holds a reference to the original root node; `m.insert()` creates a new root.
The loop iterates over the original root, unaffected by the rebinding.

### 5. Erased-value ABI (inherited from ADR 0015)

Same as ADR 0015:
- Each key/value is stored as **`i8*`** (opaque pointer to GC-boxed value)
- Box is allocated via `GC_malloc` and contains standard in-memory representation
- Boehm GC scans all i8* pointers and preserves shared nodes
- No manual free (ADR 0007 alignment)

**Compiler-generated functions**:
```llvm
define i1 @tyra_eq_<ty>(i8* %a, i8* %b) { ... }
define i64 @tyra_hash_<ty>(i8* %a) { ... }
```

HAMT traversal uses `hash_fn(key)` to extract the 64-bit hash; 5-bit chunks index levels.

### 6. Garbage collection and shared nodes

HAMT nodes are referenced by multiple Map/Set instances (via structural sharing).

**Boehm GC safety**:
- Boehm GC scans all root bindings (local variables, globals, heap pointers)
- It follows all i8* pointers and marks reachable objects
- Shared child nodes are reached via multiple paths (from different root nodes)
- Boehm marks them exactly once; they are not freed until all referencing roots are collected
- No explicit refcounting needed; no manual free

Example:
```
m1 := {1: "a", 2: "b", 3: "c", 4: "d"}  // tree with root R1 and subtrees S1, S2, S3, ...
m2 := m1.insert(5, "e")                // tree with root R2; R2 links to S1, S2, S3, ... (shared)
                                       // only new path from R2 to leaf with key 5 is copied
                                       // Boehm scans m1 → R1 → S1, S2, ...
                                       //        scans m2 → R2 → S1, S2, ... (marked again, ok)
                                       // S1, S2, ... marked reachable via both roots
```

### 7. Map/Set literal lowering

**v0.6.0 lowering** (open-addressing):
```tyra
let m = {1: "a", 2: "b"}
// → __map_new_int_string() + __map_insert_int_string(m, 1, "a") + __map_insert_int_string(m, 2, "b")
// (in-place mutations)
```

**v0.7.0 lowering** (HAMT persistent):
```tyra
let m = {1: "a", 2: "b"}
// → let m0 = map.new()
// → let m1 = m0.insert(1, "a")
// → let m2 = m1.insert(2, "b")
// → m = m2
// (chained returns)
```

The lowering uses the new **persistent insert** API.

Similarly for Set:
```tyra
let s = set.new()
s = s.insert(1)
s = s.insert(2)
```

## Alternatives considered

### A. Keep v0.6.0 in-place + accept aliasing semantics

**Rationale against**: Violates the spec ("non-destructive from language perspective").
Once iteration is added, aliasing becomes observable: two variables refer to same mutable table,
iteration can see updates from elsewhere. This is **explicit mutability** without explicit `mut` keyword.
Violates AGENTS.md principle "Explicitness over implicitness."

**Rejected.**

### B. Add `mut` keyword for explicit destructive updates

**Rationale against**: Would require spec change (§17.3.6-7).
Breaking change to v0.6.0 API: `s.insert(x)` currently returns `Set<T>` (immutable in spec);
requiring `mut s` would break existing code.
Also increases language surface area.

**Rejected.**

### C. Copy-on-write with full table copy per update

**Rationale against**: Every insert/remove would copy the entire hash table.
Time: O(n), space: O(n). For Maps with thousands of entries, each update becomes expensive.
HAMT: O(log₃₂ n) ≈ O(1) time and space (path-copy only ~7 nodes).

**Rejected.**

### D. HAMT is chosen

**Rationale**: True structural immutability. Path-copy ensures O(log₃₂ n) per operation.
Iteration safety guaranteed. Boehm GC naturally handles shared nodes.
No spec change required; v0.6.0 API signature preserved.

**Accepted.**

## Consequences

**Positive**

- **Aliasing hazard eliminated**: Rebinding a variable no longer affects other variables
  ```tyra
  let m = {1: "a"}
  let m2 = m
  m = m.insert(2, "b")
  // m2 is still {1: "a"}, not {1: "a", 2: "b"}
  ```
- **Iteration safety**: `for k, v in m` cannot be affected by updates to an alias
- **Spec compliance**: Language semantics ("non-destructive") now matches runtime behavior
- **Deterministic behavior**: AGENTS.md principle satisfied
- **Shared structure**: Memory usage benefits from structural sharing;
  1000-entry Map + insert = only ~7 new nodes, rest shared

**Negative / accepted tradeoffs**

- **Performance**: O(log₃₂ n) per operation vs. O(1) amortized for open-addressing hash table
  - For small Maps (< 100 entries), difference is negligible
  - For large Maps (10,000+ entries), cost becomes noticeable
  - Mitigated by structural sharing (most memory is shared, not copied)
  - Known Limitation in spec: users should be aware of asymptotic cost

- **Implementation complexity**: HAMT traversal, path-copy reconstruction, Collision nodes
  - More complex than open-addressing
  - Requires careful testing of path-copy logic and GC interaction

**Implementation strategy**

1. `runtime/src/stdlib_map.rs` refactored to HAMT (Bitmap, Leaf, Collision node types)
2. Path-copy insert algorithm (hashcode extraction, traversal, node copying, backtrack)
3. Minimal vertical slice: `Map<Int,Int>` end-to-end with existing compiler-generated hash/eq
4. Integration with erased-value ABI from ADR 0015 (cast to i8*, store in HAMT nodes)
5. Set implementation (reuses HAMT nodes, no separate value pointer)
6. Iteration via DFS callback (compiler generates loop that calls callback on each leaf)
7. E2E tests: Map/Set aliasing, iteration during mutation, large Maps

**Spec changes**

§17.3.6 (Map) and §17.3.7 (Set): Update implementation note to specify HAMT, path-copy semantics.
No change to surface syntax or ability constraints.

