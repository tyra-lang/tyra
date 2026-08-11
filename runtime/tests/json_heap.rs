//! Integration tests for the JSON GC-node tree's heap-verification
//! guarantees (Phase 3 permanent-leak elimination, `runtime/src/stdlib_json.rs`).
//!
//! These tests force a full `GC_gcollect()` stop-the-world cycle (via
//! `tyra_gc_collect()`). On Darwin, Boehm GC's stop-the-world reliably
//! aborts (`thread_suspend failed`) or hangs the moment it tries to
//! suspend a second, unrelated libtest per-test thread that is still
//! resident in the process (verified empirically) — or, worse, a
//! thread that was never GC-registered at all (e.g. a persistent server
//! thread spawned by an unrelated test in the same binary). Putting them
//! in a `cfg(test)` module inside the library crate meant they shared a
//! process with every other lib test, including `stdlib_http_server`'s
//! tests, which spawn exactly such threads.
//!
//! Cargo integration test files under `runtime/tests/` each compile to
//! their own binary and run in their own process, so these tests need no
//! `#[ignore]`: the only threads present are this test's own and whatever
//! `tyra_rt_init()`'s scheduler spawns (all GC-registered).
//!
//! Both checks below live in a *single* `#[test]` function rather than two.
//! Cargo's default test harness runs every `#[test]` in a binary
//! concurrently, one OS thread per test — and that sibling thread is a
//! plain libtest worker, never GC-registered. Splitting these into two
//! `#[test]` fns was tried first and reproduced the exact
//! `thread_suspend failed` / `SIGABRT` this move was meant to fix: while
//! one test's `tyra_gc_collect()` stopped the world, the other test's
//! still-alive (or mid-teardown) unregistered thread couldn't be
//! suspended. A single test function guarantees this binary never has a
//! second concurrent test thread for `GC_gcollect()` to trip over — the
//! same "lone first call always succeeds" shape verified empirically in
//! the original `#[ignore]`d lib tests this file replaces.

use std::ffi::{CStr, CString};
use tyra_runtime::{
    gc_is_gc_ptr, tyra_gc_collect, tyra_gc_heap_size, tyra_json_get, tyra_json_is_string,
    tyra_json_parse, tyra_json_str, tyra_rt_init,
};

fn cstr(s: &str) -> CString {
    CString::new(s).unwrap()
}

/// Regression guard for the Box::leak design this module replaced: that
/// implementation grew the heap by roughly `10_000 * tree_size` bytes
/// (tens of MB for a ~1KB document) because every successful parse
/// permanently leaked its root. Under the GC-node design, a parsed tree
/// becomes ordinary reclaimable garbage once its handle is no longer
/// reachable, so heap growth across many parse/drop cycles should stay
/// small relative to that old baseline.
fn heap_growth_bounded_after_many_parses() {
    tyra_rt_init();

    // ~1KB JSON document: 40 key/value pairs with padded string values.
    let mut doc = String::from("{");
    for i in 0..40 {
        if i > 0 {
            doc.push(',');
        }
        doc.push_str(&format!(r#""key{i}":"value number {i} padding padding""#));
    }
    doc.push('}');
    assert!(
        doc.len() > 900,
        "test document should be roughly 1KB, got {} bytes",
        doc.len()
    );
    let input = cstr(&doc);

    // Regression probe: prove the returned handle is actually backed by
    // the Boehm heap, not the Rust system allocator. The old design this
    // module replaced (`Box::leak`) also returned a non-null, non-zero
    // `i64` — and would *still* pass the heap-growth-bounded assertion
    // below, because growth on the system allocator is invisible to
    // `tyra_gc_heap_size` (verified empirically: 10k `Box::leak`s of the
    // same size show zero growth here). Without this assertion, reverting
    // the leak fix would silently pass this whole test.
    let root_h = unsafe { tyra_json_parse(input.as_ptr()) };
    assert!(root_h != 0);
    assert!(
        gc_is_gc_ptr(root_h),
        "handle must be a GC pointer, not a system-allocator leak"
    );

    // Warm up: let the allocator reach a steady state before measuring, so
    // first-time page-fault / arena-growth overhead isn't counted against
    // the bound below.
    for _ in 0..100 {
        let h = unsafe { tyra_json_parse(input.as_ptr()) };
        assert!(h != 0);
    }
    tyra_gc_collect();
    let before = tyra_gc_heap_size();

    for _ in 0..10_000 {
        let h = unsafe { tyra_json_parse(input.as_ptr()) };
        assert!(h != 0);
        // `h` is a plain i64; the handle is simply not carried forward, so
        // nothing keeps this iteration's tree reachable afterwards.
    }
    tyra_gc_collect();
    let after = tyra_gc_heap_size();

    let growth = after.saturating_sub(before);
    assert!(
        growth < 8 * 1024 * 1024,
        "heap grew by {growth} bytes after 10k dropped parses; expected \
         bounded growth (the old Box::leak design would grow roughly \
         10k x tree size here)"
    );
}

/// Parse a small nested document and return only the innermost child's
/// handle — `root_h`/`a_h`/`b_h` are local to this frame and never
/// propagate to the caller.
///
/// `#[inline(never)]` matters here, not just as documentation: this frame
/// must actually be popped off the stack (and its slots become eligible
/// to be overwritten by `scrub_stack()`) once this function returns. If
/// the compiler inlined this into the caller, `root_h`/`a_h`/`b_h` would
/// occupy the *same* frame as the assertions that follow — exactly the
/// "stale i64 stack slots in the same frame" hazard this split exists to
/// avoid (a conservative scanner cannot tell a live handle from a dead
/// one still sitting in memory, so it would root all four handles and the
/// test could not attribute survival to `child_h` specifically).
#[inline(never)]
fn acquire_deep_child(input: &CString) -> i64 {
    let root_h = unsafe { tyra_json_parse(input.as_ptr()) };
    assert!(root_h != 0);
    let a = cstr("a");
    let b = cstr("b");
    let c = cstr("c");
    let a_h = unsafe { tyra_json_get(root_h, a.as_ptr()) };
    let b_h = unsafe { tyra_json_get(a_h, b.as_ptr()) };
    unsafe { tyra_json_get(b_h, c.as_ptr()) }
}

/// Overwrite the stack memory `acquire_deep_child`'s now-dead frame
/// occupied, so `root_h` / `a_h` / `b_h` cannot still be sitting
/// byte-for-byte in memory a conservative scan would walk. `black_box`
/// and `#[inline(never)]` stop the optimizer from proving the buffer is
/// unused and eliding the whole function; ~16KB comfortably exceeds a
/// single test frame on every platform this suite runs on.
#[inline(never)]
fn scrub_stack() {
    let buf = [0u8; 16 * 1024];
    std::hint::black_box(&buf);
}

/// Validates the "no finalizers" design: a child handle obtained via
/// `tyra_json_get` is itself a GC pointer, so it roots its own subtree
/// independently of the root handle it was reached through.
///
/// Conservative GC admits no formal proof of liveness from Rust-level
/// scoping alone — a stack scanner cannot see Rust's lexical scopes, only
/// raw memory, so a value that has "gone out of scope" can still be
/// found and rooted if its old stack slot has not been overwritten. This
/// test is therefore a strong *probe*, not a guarantee: `acquire_deep_child`
/// isolates `root_h`/`a_h`/`b_h` to a frame that actually gets popped,
/// `scrub_stack` overwrites what that frame left behind, and the 20k-parse
/// churn loop below forces the allocator to recycle blocks so any residual
/// copy of the old handles is maximally likely to have been reused for
/// something else by the time we assert `child_h` is still readable.
fn child_survives_after_root_handle_goes_out_of_scope() {
    tyra_rt_init();
    let input = cstr(r#"{"a":{"b":{"c":"deep value"}}}"#);

    let child_h = acquire_deep_child(&input);
    assert!(child_h != 0);
    assert!(
        gc_is_gc_ptr(child_h),
        "handle must be a GC pointer, not a system-allocator leak"
    );

    scrub_stack();
    tyra_gc_collect();

    // Force block reuse: many small parse/drop cycles so the allocator
    // has ample opportunity to recycle whatever memory `root_h`/`a_h`/
    // `b_h` occupied, if `child_h` were not genuinely rooting its own
    // subtree independently of them.
    let churn_input = cstr(r#"{"x":1}"#);
    for _ in 0..20_000 {
        let h = unsafe { tyra_json_parse(churn_input.as_ptr()) };
        assert!(h != 0);
    }
    tyra_gc_collect();

    assert_eq!(unsafe { tyra_json_is_string(child_h) }, 1);
    let s = unsafe { CStr::from_ptr(tyra_json_str(child_h)) }
        .to_str()
        .unwrap();
    assert_eq!(s, "deep value");
}

/// Single entry point so both GC-heavy checks above run sequentially on
/// one thread — see the module doc for why splitting them into separate
/// `#[test]` fns is unsafe under Cargo's default parallel test harness.
#[test]
fn json_heap_verification() {
    heap_growth_bounded_after_many_parses();
    child_survives_after_root_handle_goes_out_of_scope();
}
