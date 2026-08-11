//! Integration tests for the HTTP client's GC-managed `HttpResponse`
//! heap-verification guarantees (Phase 3 permanent-leak elimination,
//! `runtime/src/stdlib_http.rs`).
//!
//! Same rationale as `runtime/tests/json_heap.rs`: these tests force a
//! full `GC_gcollect()` cycle (via `tyra_gc_collect()`), which reliably
//! aborts on Darwin once other, unrelated threads are resident in the
//! same process — in particular the persistent server threads spawned by
//! `stdlib_http_server`'s tests, which are never GC-registered and so
//! cannot be suspended by the collector. Living in their own integration
//! test binary sidesteps that; no `#[ignore]` needed.
//!
//! Both checks below live in a *single* `#[test]` function rather than
//! two, for the same reason documented in `json_heap.rs`: Cargo's default
//! harness runs every `#[test]` in a binary concurrently on its own OS
//! thread, and that sibling thread is never GC-registered. Splitting
//! these into two `#[test]` fns was tried first and reproduced
//! `thread_suspend failed` / `SIGABRT` — the exact failure this move was
//! meant to eliminate.

use std::ffi::CStr;
use tyra_runtime::{
    alloc_response, gc_is_gc_ptr, tyra_gc_collect, tyra_gc_heap_size, tyra_http_body,
    tyra_http_status, tyra_rt_init,
};

fn alloc_response_roundtrips_through_accessors() {
    tyra_rt_init();
    let resp_h = alloc_response(200, "hello");
    assert!(resp_h != 0);
    // Regression probe: the old `Box::leak` design also returned a
    // non-null, non-zero handle and would still pass the heap-growth
    // bound in the sibling check below (growth on the system allocator is
    // invisible to `tyra_gc_heap_size`). Assert the handle is actually a
    // Boehm-GC pointer so a reverted leak fix cannot pass this test
    // silently.
    assert!(
        gc_is_gc_ptr(resp_h),
        "handle must be a GC pointer, not a system-allocator leak"
    );
    assert_eq!(unsafe { tyra_http_status(resp_h) }, 200);
    let body_ptr = unsafe { tyra_http_body(resp_h) };
    let body = unsafe { CStr::from_ptr(body_ptr) }.to_str().unwrap();
    assert_eq!(body, "hello");
}

/// Regression guard for the Box::leak design this module replaced: that
/// implementation permanently leaked one `HttpResponse` per successful
/// request. Under the GC design, an allocated response becomes ordinary
/// reclaimable garbage once its handle is no longer reachable, so heap
/// growth across many alloc/drop cycles should stay small relative to
/// that old baseline. No network I/O — this exercises `alloc_response`
/// directly.
fn heap_growth_bounded_after_many_responses() {
    tyra_rt_init();
    let body = "warm up response body padding padding padding padding";

    for _ in 0..100 {
        let h = alloc_response(200, body);
        assert!(h != 0);
    }
    tyra_gc_collect();
    let before = tyra_gc_heap_size();

    for _ in 0..10_000 {
        let h = alloc_response(200, body);
        assert!(h != 0);
        // `h` is a plain i64, not carried forward past this iteration.
    }
    tyra_gc_collect();
    let after = tyra_gc_heap_size();

    let growth = after.saturating_sub(before);
    assert!(
        growth < 8 * 1024 * 1024,
        "heap grew by {growth} bytes after 10k dropped responses; \
         expected bounded growth (the old Box::leak design would grow \
         roughly 10k x response size here)"
    );
}

/// Single entry point so both checks above run sequentially on one
/// thread — see the module doc for why splitting them into separate
/// `#[test]` fns is unsafe under Cargo's default parallel test harness.
#[test]
fn http_heap_verification() {
    alloc_response_roundtrips_through_accessors();
    heap_growth_bounded_after_many_responses();
}
