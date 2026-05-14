# 050 bank-account double-match bug — diagnosis

## Symptom

```tyra
let r = get(500)   # returns Err(BankError.Overdraft)

let a = match r
when Ok(v)  -> v
when Err(_) -> fallback   # expects fallback (struct), gets garbage
end
```

`a` gets garbage instead of `fallback`. Second `match r` on same value works correctly.

Minimal repro: `/tmp/050_min8.tyra`

## Root cause — chain of events

### Step 1: Parser (`tyra-parser/src/pattern.rs:141`)

`Err(_)` is parsed in `parse_pattern_fields`. `_` is lexed as
`TokenKind::Ident("_")`, so it takes the **shorthand identifier path** (not the
non-identifier path at line 195). Result:

```
PatternField { field_name: "_", pattern: PatternKind::Ident("_") }
```

### Step 2: `rename_pattern_bindings` (`tyra-driver/src/lib.rs:628`)

This pass alpha-renames every `PatternKind::Ident(name)` to `{name}__p{N}`, then
updates `field_name` to match. It has **no special case for `"_"`**, so:

```
"_"  →  "___p2"    (format!("{orig}__p{counter}") with orig="_")
PatternField { field_name: "___p2", pattern: PatternKind::Ident("___p2") }
```

### Step 3: `match_lower.rs:521` — guard bypassed

The prelude payload-binding guard:

```rust
if is_prelude && !fields.is_empty()
    && fields[0].field_name != "_"   // ← intended to skip wildcards
    && !inner_is_constructor
    && !inner_is_literal              // ← only lists Wildcard, not Ident
```

By the time match_lower sees it, `field_name == "___p2"` ≠ `"_"` → guard passes.
`inner_is_literal` also misses it because `Ident("___p2")` is not `Wildcard`.

Payload binding is emitted:

```
AdtPayload { dest: "___p2", src: r, field: "err_val" }
Store      { dest: "___p2", value: "___p2" }
```

### Step 4: Arm body emits no instruction

`arm_body_start` is set to `body.len()` **before** payload binding (line ~499).
The arm body `fallback` is a plain `let` binding (not in `pattern_vars`), so
`ExprKind::Ident("fallback")` returns the name with no MIR instruction.

Arm range (2 instructions):
```
[arm_body_start]
  Store { dest: "___p2", value: "___p2" }   ← only instruction in range
  (nothing from `fallback`)
[arm_body_end]
```

### Step 5: `block_ends_with_assignment` returns `true`

```rust
// mod.rs:1478
Instruction::Store { dest, .. } => {
    let is_temp = dest.starts_with("_t");
    let is_defer_flag = dest.starts_with(".defer_active_");
    return !is_temp && !is_defer_flag;   // "___p2" → returns true
}
```

`"___p2"` starts with neither `"_t"` nor `".defer_active_"` → returns `true`.

### Step 6: Arm result not stored → garbage output

Because `block_ends_with_assignment` returned `true`, the code that stores
`fallback` into the match result slot is skipped. The result slot keeps its
previous (uninitialized or stale) value.

## Debug trace (confirmed with eprintln on HEAD)

```
[DBG arm 0] pattern=Constructor("Ok", [PatternField { field_name: "v__p1", ... }])
            tail=Value("_t19") terminates=false ends_assign=false range_len=3
[DBG arm 1] pattern=Constructor("Err", [PatternField { field_name: "___p2", ... }])
            tail=Value("fallback") terminates=false ends_assign=true range_len=2
```

- Arm 0 (`Ok(v)`): arm body `v__p1` is in `pattern_vars` → emits `Load { dest: "_t19" }`.
  `block_ends_with_assignment` hits the `Load`, falls through to `_ => return false`. OK.
- Arm 1 (`Err(_)`): arm body `fallback` emits nothing. Backward scan hits
  `Store { dest: "___p2" }` → returns `true`. **BUG.**

## Two compounding defects

| # | Location | Defect |
|---|----------|--------|
| A | `tyra-driver/src/lib.rs:644` | `rename_pattern_bindings` renames `_` (wildcard discard) to `___pN`, defeating the `field_name != "_"` guard in match_lower |
| B | `tyra-mir/src/lower/mod.rs:1478` | `block_ends_with_assignment` cannot distinguish payload-binding stores from user-assignment stores |

Either fix alone resolves the bug; fixing both is belt-and-suspenders.

## Fix directions (Phase 2)

### Fix A — stop renaming `"_"` in `rename_pattern_bindings`
In `collect_idents` (`lib.rs:644`), skip when `name == "_"`:
```rust
PatternKind::Ident(name) if name != "_" => { ... }   // skip wildcard
```
This restores `field_name == "_"` so the match_lower guard at line 522 catches it.
Also naturally fixes `inner_is_literal` coverage (pattern stays `Ident("_")` → but
the guard fires first anyway).

### Fix B — use `arm_payload_end`
In `match_lower.rs`, record `let arm_payload_end = body.len()` **after** the payload
binding block, and pass `arm_payload_end` (not `arm_body_start`) to
`block_ends_with_assignment`. Payload-binding stores are excluded from the scan.

### Recommendation
**Fix B is mandatory.** The structural defect — `block_ends_with_assignment` cannot
distinguish payload-binding stores from user-assignment stores — is a general problem.
Even with Fix A applied, a pattern like `when Err(e) -> fallback` (named payload
binding, arm body is a bare ident that emits no instruction) would still leave
`Store { dest: "e__pN" }` as the last instruction in the arm range, triggering the
same misidentification.

**Fix A is complementary.** `_` is a discard, not a binding, and should not be
renamed. Applying A prevents unnecessary payload binding emission for wildcards and
restores the `field_name != "_"` guard. However, A alone does not close the general
case.

Implement B first (the range-boundary fix in MIR lowering); implement A alongside it
as a semantic correctness fix.

## Affected files

- `compiler/crates/tyra-driver/src/lib.rs:644` (Fix A — 2 lines)
- `compiler/crates/tyra-mir/src/lower/match_lower.rs` (Fix B if applied — ~3 lines)
