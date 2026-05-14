# 099 sum-column SIGSEGV — diagnosis

## Symptom

Run 51 `099-sum-column__tyra+spec__claude__s1`: the model generated a `while true`
loop with no `break` that kept calling `io.read_line()` past EOF. The program
compiled successfully (exit 0) but crashed with **SIGSEGV (-11 / exit code 139)**
immediately — stdout and stderr both empty, timeout never reached.

## Minimal repro

```tyra
import io

fn main() -> Unit
  mut line_opt = io.read_line()
  while true
    match line_opt
    when None
      line_opt = io.read_line()
    when Some(_line)
      line_opt = io.read_line()
    end
  end
end
```

Run with `printf 'a 1\n' | ./prog` → exits 139 (SIGSEGV) immediately.

## Control experiments

| Program | Behaviour | Conclusion |
|---|---|---|
| Pure spin loop (`while true; i = i+1; end`) | Hangs (timeout) | No segv without allocations |
| EOF-aware loop (breaks on `None`) | Exits normally (exit 0) | Read-line itself is fine |
| `while true` calling `read_line()` | **SIGSEGV** | Unbounded allocation in loop is trigger |

## Root cause — alloca in non-entry basic block

### LLVM IR evidence

`tyra emit-ir` shows the `while_0_body` basic block of `main` contains:

```llvm
while_0_body:
  %_t2 = load %struct.Option__String, ptr %line_opt
  %_t3 = alloca i64           ; ← alloca INSIDE the loop body
  ...
match_end_2:
  %_t13 = load i64, ptr %_t3  ; loaded but never stored — dead alloca
  br label %while_0
```

`%_t3` is allocated fresh each time `while_0_body` is entered.

### LLVM alloca semantics

Per the LLVM language reference, `alloca` in a non-entry basic block allocates
on the stack frame every time the block is executed and is **never freed until
the function returns**. This causes unbounded O(iterations) stack growth in
infinite loops.

Each iteration allocates at least 8 bytes (i64) — plus alignment padding to 16
bytes on arm64 — for this dead alloca. After O(2^17) iterations (~130k) the
8 MiB default stack is exhausted. The stack overflow manifests as
`EXC_BAD_ACCESS (code=2, address=0x16f6...ff0)` — a guard page write fault —
not inside the loop itself but inside `CString::new` (via `leak_cstring` →
`tyra_io_read_line`) where `alloc::raw_vec::RawVecInner::finish_grow` touches
the stack during a Vec reserve operation.

### lldb backtrace (arm64 macOS)

```
frame #0:  alloc::raw_vec::RawVecInner::finish_grow  — guard page fault
frame #1:  alloc::raw_vec::RawVecInner::grow_exact
...
frame #9:  tyra_runtime::stdlib_io::leak_cstring (stdlib_io.rs:48)
frame #10: tyra_io_read_line (stdlib_io.rs:65)
frame #11: io__read_line + 12
frame #12: main + 140
```

The crash is in Rust's allocator trying to grow a `Vec` for `CString::new`,
not in Boehm GC. The GC/malloc mismatch hypothesis (from static analysis) was
**incorrect** — there is no false reclamation occurring.

### Why the dead alloca appears

The codegen emits an `alloca` for every match-expression result even when the
result is not used (Unit arms, void-returning matches). These should be hoisted
to the function entry block (`entry:`) — the standard LLVM practice — so they
are allocated once and reused across iterations.

Note: `io__read_line()` also contains an `alloca %struct.Option__String` inside
it, but since that is a called function (not a loop in the same frame), its
stack frame is properly cleaned up on return. The problem is limited to allocas
in the *caller's* loop body.

## Scope assessment

This is a **codegen bug** (alloca in non-entry block), not a `stdlib/io.tyra`
limitation. The existing `io.tyra:18-21` warning ("avoid hot polling loops in
v0.1") is inaccurate about the cause — the issue is not GC or allocation
lifetime but stack exhaustion from loop-body allocas. Any unbounded loop
that generates match-expression allocas will eventually overflow, regardless
of whether `io.read_line` is involved.

## Recommended fix

Hoist all `alloca` instructions to the function entry block in the LLVM codegen
(`compiler/crates/tyra-codegen-llvm/`). Two places to look:

1. **Match-expression result allocas** — the codegen emits `alloca T` for the
   result variable of each `match` expression. These should be collected during
   function lowering and emitted in the `entry:` block rather than inline at
   first use.
2. **Other per-expression allocas** — audit for any other `alloca` emitted
   outside `entry:` (e.g. `io__read_line` itself emits `alloca` inside an
   `if`-then-else, though that function is fine since it returns normally).

Standard LLVM practice: emit all `alloca` instructions at the top of the
`entry` basic block before any other instructions, reusing the same slot across
loop iterations.

## Status

- `stdlib/io.tyra:18-21` comment ("avoid hot polling loops") should be updated
  to point to the alloca-in-loop-body root cause rather than implying GC.
- `runtime/src/stdlib_io.rs:20-23` comment ("scanned conservatively by Boehm
  GC") is factually incorrect (no `#[global_allocator]` replacement; GC does
  not manage system-malloc'd CString buffers) and should be corrected in a
  follow-up cleanup.
- The codegen fix (alloca hoisting) is deferred — tracked here as a known bug.
