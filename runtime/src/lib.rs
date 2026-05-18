//! Tyra runtime library (v0.1).
//!
//! Provides the C ABI that Tyra-compiled programs link against:
//! - `tyra_rt_init` / `tyra_rt_shutdown` — scheduler lifecycle
//! - `tyra_task_spawn` / `tyra_task_await` / `tyra_task_join_all` — task API
//!
//! The scheduler is a fixed-size thread pool of size `available_parallelism`
//! (fallback 4). Tasks are trampolined through a `thunk(arg) -> result` C
//! function pointer produced by the LLVM codegen. The runtime never
//! interprets `arg` or `result` pointers — they are opaque values owned by
//! the compiler's allocation scheme.
//!
//! Task ownership is tracked via `Arc<Task>`, shared between the worker
//! thread and the awaiter, so neither side can free the task prematurely.
//! Handles crossing the C ABI are `Arc::into_raw` / `Arc::from_raw` pairs.
//!
//! Boehm GC integration is unconditional: worker threads register with
//! the collector via `GC_register_my_thread` so their stacks are included
//! in conservative scans (ADR-0007). The runtime's `build.rs` arranges
//! for `-lgc` to be passed at link time for both the staticlib and any
//! downstream Rust test binary, so no Cargo feature toggling is needed.

use crossbeam_channel::{Receiver, Sender, unbounded};
use std::os::raw::c_void;
use std::ptr;
use std::sync::{Arc, Condvar, Mutex, OnceLock};
use std::thread;

mod gc_string;

mod stdlib_fs;
pub use stdlib_fs::{
    tyra_fs_errmsg, tyra_fs_errno, tyra_fs_exists, tyra_fs_read, tyra_fs_write,
};

mod stdlib_json;
pub use stdlib_json::{
    tyra_json_at, tyra_json_bool, tyra_json_err_col, tyra_json_err_line, tyra_json_err_msg,
    tyra_json_get, tyra_json_int, tyra_json_is_bool, tyra_json_is_int, tyra_json_is_string,
    tyra_json_kind, tyra_json_parse, tyra_json_str,
};

mod stdlib_http;
pub use stdlib_http::{
    tyra_http_body, tyra_http_errmsg, tyra_http_errno, tyra_http_get, tyra_http_status,
};

mod stdlib_http_server;
pub use stdlib_http_server::{
    tyra_http_server_listen, tyra_http_server_new, tyra_http_server_route,
};

mod stdlib_io;
pub use stdlib_io::{tyra_io_eof, tyra_io_read_line, tyra_io_read_to_end};

mod stdlib_map;
pub use stdlib_map::{
    tyra_map_contains_string_int, tyra_map_get_present, tyra_map_get_string_int,
    tyra_map_insert_string_int, tyra_map_new_string_int,
};

mod stdlib_string;
pub use stdlib_string::{
    tyra_string_byte_at, tyra_string_contains, tyra_string_ends_with, tyra_string_from_byte,
    tyra_string_is_empty, tyra_string_len, tyra_string_parse_errno, tyra_string_parse_int,
    tyra_string_reverse, tyra_string_split, tyra_string_split_whitespace, tyra_string_starts_with,
    tyra_string_substring, tyra_string_to_lower, tyra_string_to_upper, tyra_string_trim,
};

pub mod stdlib_float;
pub use stdlib_float::{
    tyra_float_abs, tyra_float_approx_eq, tyra_float_ceil, tyra_float_eq, tyra_float_floor,
    tyra_float_from_int, tyra_float_is_infinite, tyra_float_is_nan, tyra_float_max,
    tyra_float_min, tyra_float_parse, tyra_float_parse_errno, tyra_float_round,
    tyra_float_to_int, tyra_float_to_string,
};

/// Thunk signature emitted by codegen for each `spawn` site.
/// Takes opaque arg pointer, returns opaque result pointer.
pub type ThunkFn = unsafe extern "C" fn(*mut c_void) -> *mut c_void;

/// A task handle. Opaque to codegen; lifetime managed via `Arc`.
pub struct Task {
    inner: Mutex<TaskInner>,
    cond: Condvar,
}

/// Mutable state guarded by `Task::inner`. `waiters` collects senders
/// registered by `tyra_task_select` so that any select call observing
/// `Pending` gets woken the instant this task transitions to `Done`.
struct TaskInner {
    state: TaskState,
    waiters: Vec<Sender<*mut c_void>>,
}

enum TaskState {
    Pending,
    Done(*mut c_void),
}

// SAFETY: `Done(*mut c_void)` holds a pointer produced by a user thunk. We
// move it from worker to awaiter exactly once, under the mutex.
unsafe impl Send for TaskInner {}
unsafe impl Sync for Task {}
unsafe impl Send for Task {}

struct Job {
    thunk: ThunkFn,
    arg: *mut c_void,
    task: Arc<Task>,
}

// SAFETY: the raw `arg` pointer is produced by the Tyra compiler and is
// guaranteed to outlive the task (it is either a GC-managed allocation or
// an integer bit-cast to a pointer). Send is safe by contract.
unsafe impl Send for Job {}

struct Scheduler {
    sender: Sender<Job>,
    _workers: Vec<thread::JoinHandle<()>>,
}

impl Scheduler {
    fn new(num_workers: usize) -> Self {
        let (sender, receiver) = unbounded::<Job>();
        let mut workers = Vec::with_capacity(num_workers);
        for _ in 0..num_workers {
            let rx: Receiver<Job> = receiver.clone();
            let handle = thread::spawn(move || worker_loop(rx));
            workers.push(handle);
        }
        Scheduler {
            sender,
            _workers: workers,
        }
    }
}

fn worker_loop(rx: Receiver<Job>) {
    gc::register_this_thread();
    while let Ok(job) = rx.recv() {
        let result = unsafe { (job.thunk)(job.arg) };
        let mut inner = job.task.inner.lock().unwrap();
        inner.state = TaskState::Done(result);
        // Notify any `tyra_task_select` callers that subscribed while we
        // were still Pending. Draining happens under the same mutex that
        // writes `state`, so the select side either sees Done directly
        // (no subscription needed) or receives a message here.
        for w in inner.waiters.drain(..) {
            let _ = w.send(result);
        }
        job.task.cond.notify_all();
        // `inner` drops here, releasing the mutex. `job` drops at end of
        // loop body, decrementing the Arc refcount. The awaiter's Arc keeps
        // the Task alive long enough for it to observe `Done`.
        drop(inner);
    }
    gc::unregister_this_thread();
}

static SCHEDULER: OnceLock<Scheduler> = OnceLock::new();

fn scheduler() -> &'static Scheduler {
    SCHEDULER
        .get()
        .expect("tyra_rt_init must be called before using the task API")
}

fn default_worker_count() -> usize {
    thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(4)
}

// ---------------------------------------------------------------------------
// Boehm GC thread registration.
// ---------------------------------------------------------------------------

mod gc {
    use std::os::raw::c_void;
    use std::sync::Once;

    #[repr(C)]
    struct GCStackBase {
        mem_base: *mut c_void,
        reg_base: *mut c_void,
    }

    unsafe extern "C" {
        fn GC_init();
        fn GC_allow_register_threads();
        fn GC_get_stack_base(sb: *mut GCStackBase) -> i32;
        fn GC_register_my_thread(sb: *const GCStackBase) -> i32;
        fn GC_unregister_my_thread() -> i32;
        fn GC_malloc_atomic(size: usize) -> *mut c_void;
    }

    pub(crate) fn malloc_atomic(size: usize) -> *mut c_void {
        // GC_malloc_atomic requires GC_init to have been called first.
        // init() is Once-guarded so this is a no-op after the first call.
        init();
        unsafe { GC_malloc_atomic(size) }
    }

    static INIT: Once = Once::new();

    /// Idempotent libgc initialization. Tyra-compiled binaries call
    /// `GC_init` from their generated main (ADR-0007), but Rust-side
    /// consumers (runtime unit tests) never enter that main. Calling
    /// `GC_init` here is safe — libgc documents it as idempotent — and
    /// `GC_allow_register_threads` is required before the scheduler's
    /// worker threads can register via `GC_register_my_thread`.
    pub(crate) fn init() {
        INIT.call_once(|| unsafe {
            GC_init();
            GC_allow_register_threads();
        });
    }

    pub(crate) fn register_this_thread() {
        unsafe {
            let mut sb = GCStackBase {
                mem_base: std::ptr::null_mut(),
                reg_base: std::ptr::null_mut(),
            };
            // GC_get_stack_base returns 0 on success (GC_SUCCESS); any
            // other value (typically GC_UNIMPLEMENTED = 2) means the
            // platform cannot report a stack base and conservative scans
            // will rely on whatever default libgc computed.
            if GC_get_stack_base(&mut sb) == 0 {
                GC_register_my_thread(&sb);
            }
        }
    }

    pub(crate) fn unregister_this_thread() {
        unsafe {
            GC_unregister_my_thread();
        }
    }
}

// ---------------------------------------------------------------------------
// C ABI
// ---------------------------------------------------------------------------

/// Initialize the runtime. Idempotent; second call is a no-op.
///
/// Must be invoked from `main` before any `tyra_task_spawn`. The call
/// order inside this function is load-bearing:
///   1. `gc::init()` — runs `GC_init` + `GC_allow_register_threads` via
///      a `Once` guard. `GC_allow_register_threads` must run before any
///      thread that will later call `GC_register_my_thread` is spawned.
///   2. Scheduler construction spawns worker threads; each worker's
///      first action is `GC_register_my_thread`. Reversing (1) and (2)
///      would race the workers against libgc's thread-registration
///      allowlist — undefined behavior per the Boehm GC spec.
///
/// Thread-safety: concurrent calls are safe. Both `gc::init`'s `Once`
/// and the scheduler's `OnceLock` block redundant callers until the
/// first one finishes, providing a happens-before to all observers.
#[unsafe(no_mangle)]
pub extern "C" fn tyra_rt_init() {
    gc::init();
    let _ = SCHEDULER.get_or_init(|| Scheduler::new(default_worker_count()));
}

/// Best-effort shutdown hook. v0.1: no-op (process exit reclaims threads).
///
/// The runtime does not guarantee outstanding tasks are drained. Callers
/// must `tyra_task_await` every spawned handle before invoking shutdown,
/// or accept that pending work may be cut off at process exit.
#[unsafe(no_mangle)]
pub extern "C" fn tyra_rt_shutdown() {}

/// Submit a task for execution. Returns an opaque Task handle.
///
/// # Preconditions
/// `tyra_rt_init` must have been called first. This establishes two
/// invariants the scheduler relies on:
///   - the scheduler `OnceLock` is initialized (enforced: `scheduler()`
///     panics otherwise);
///   - libgc has seen `GC_allow_register_threads`, so the new worker
///     thread can legally call `GC_register_my_thread` (ADR-0007).
///
/// # Safety
/// `thunk` must be a valid function pointer; `arg` must be valid for the
/// lifetime of the task (typically a GC-managed argument struct).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tyra_task_spawn(thunk: ThunkFn, arg: *mut c_void) -> *const Task {
    let task = Arc::new(Task {
        inner: Mutex::new(TaskInner {
            state: TaskState::Pending,
            waiters: Vec::new(),
        }),
        cond: Condvar::new(),
    });
    let worker_task = Arc::clone(&task);
    scheduler()
        .sender
        .send(Job {
            thunk,
            arg,
            task: worker_task,
        })
        .expect("tyra scheduler channel closed");
    Arc::into_raw(task)
}

/// Block until the task completes; return its result pointer.
/// Consumes the handle (reclaims the awaiter's Arc reference).
///
/// # Safety
/// `task` must be a pointer previously returned by `tyra_task_spawn` and
/// not yet passed to `tyra_task_await` or `tyra_task_join_all`. The Tyra
/// compiler is responsible for ensuring each handle is awaited exactly
/// once; double-await is undefined behavior (Arc double-free).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tyra_task_await(task: *const Task) -> *mut c_void {
    assert!(!task.is_null(), "tyra_task_await: null task");
    let task: Arc<Task> = unsafe { Arc::from_raw(task) };
    let mut inner = task.inner.lock().unwrap();
    loop {
        match inner.state {
            TaskState::Done(result) => return result,
            TaskState::Pending => {
                inner = task.cond.wait(inner).unwrap();
            }
        }
    }
}

/// Block until all tasks in `tasks[0..n]` complete; write results in order.
///
/// # Safety
/// Each `tasks[i]` must be a valid handle from `tyra_task_spawn` not yet
/// awaited. `results` must point to at least `n` pointer-sized slots.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tyra_task_join_all(
    tasks: *const *const Task,
    n: i64,
    results: *mut *mut c_void,
) {
    assert!(n >= 0, "tyra_task_join_all: negative count");
    assert!(
        !tasks.is_null() && !results.is_null(),
        "tyra_task_join_all: null buffer"
    );
    let n = n as usize;
    for i in 0..n {
        let t = unsafe { *tasks.add(i) };
        let r = unsafe { tyra_task_await(t) };
        unsafe { ptr::write(results.add(i), r) };
    }
}

/// Spawn a dispatcher task that resolves when the first of `tasks[0..n]`
/// completes, reporting that task's result as its own. Remaining tasks
/// continue to run — v0.1 does not support cancellation (§22). Caller
/// receives a new Task handle that must be awaited exactly once via
/// `tyra_task_await`.
///
/// # Semantics
/// - If any of the source tasks is already Done when select is called,
///   the dispatcher resolves immediately with that result.
/// - Otherwise the dispatcher subscribes to each pending task via the
///   `TaskInner::waiters` channel and blocks until the first message.
///
/// # Resource accounting (caller responsibilities)
/// Losing tasks are NOT cancelled — they run to completion. The caller
/// still holds a raw Arc for each source handle and must either:
///   (a) await each source via `tyra_task_await` (consumes the Arc), or
///   (b) accept the tasks running to completion with their results
///       dropped and their Arc pinned until process exit.
/// Failing to do (a) leaks one Arc per source per select call.
///
/// # Scaling limits
/// Each select call spawns one OS dispatcher thread (outside the worker
/// pool to avoid worker-starvation deadlocks). This bounds the concurrent
/// select count by the per-process thread limit (Linux default 4096 via
/// RLIMIT_NPROC). Tight loops doing `tasks.select` at scale should chunk
/// or serialize their select calls.
///
/// # Safety
/// Each `tasks[i]` must be a valid handle produced by `tyra_task_spawn`
/// whose refcount the caller still owns (i.e. it has NOT yet been passed
/// to `tyra_task_await` / `tyra_task_join_all`). Select does not consume
/// the caller's Arc; internally it clones via `Arc::increment_strong_count`
/// so each source task stays alive until both the caller and the
/// dispatcher release it.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tyra_task_select(
    tasks: *const *const Task,
    n: i64,
) -> *const Task {
    assert!(n > 0, "tyra_task_select: empty task list");
    assert!(!tasks.is_null(), "tyra_task_select: null buffer");

    // Clone an Arc per source task so the dispatcher thread has its own
    // strong reference independent of the caller's.
    let clones: Vec<Arc<Task>> = (0..n as usize)
        .map(|i| {
            let p = unsafe { *tasks.add(i) };
            assert!(!p.is_null(), "tyra_task_select: null task handle");
            unsafe { Arc::increment_strong_count(p) };
            unsafe { Arc::from_raw(p) }
        })
        .collect();

    // Result task, owned jointly by the caller (via Arc::into_raw below)
    // and the dispatcher (via Arc::clone).
    let result = Arc::new(Task {
        inner: Mutex::new(TaskInner {
            state: TaskState::Pending,
            waiters: Vec::new(),
        }),
        cond: Condvar::new(),
    });
    let result_dispatcher = Arc::clone(&result);

    // JoinHandle is intentionally discarded: the dispatcher has no
    // failure path that the caller can act on, and joining it would
    // require an extra Arc<Mutex<Option<JoinHandle>>> with lifecycle
    // coordination. If the dispatcher panics (unreachable under the
    // current design), GC_unregister_my_thread is skipped — acceptable
    // because Tyra processes abort on panic rather than recover.
    thread::spawn(move || {
        gc::register_this_thread();
        let (tx, rx) = unbounded::<*mut c_void>();
        let mut early: Option<*mut c_void> = None;

        // Subscribe to each source task (or capture an already-Done result).
        for arc in &clones {
            let mut inner = arc.inner.lock().unwrap();
            match inner.state {
                TaskState::Done(r) => {
                    early = Some(r);
                    break;
                }
                TaskState::Pending => {
                    inner.waiters.push(tx.clone());
                }
            }
        }
        drop(tx);

        // rx.recv() can only return Err if every cloned Sender is dropped
        // without emitting a value. Under the worker_loop contract this
        // cannot happen: as long as `waiters` contained at least one
        // sender, the Done-transition drains + sends before dropping.
        // Surface as an explicit panic rather than letting a stray null
        // leak into compiled Tyra code (§7.2: Tyra has no null).
        let winning = early.unwrap_or_else(|| {
            rx.recv()
                .expect("tyra_task_select: all waiters dropped without a Done signal")
        });

        // Publish the result on the dispatcher task.
        let mut inner = result_dispatcher.inner.lock().unwrap();
        inner.state = TaskState::Done(winning);
        for w in inner.waiters.drain(..) {
            let _ = w.send(winning);
        }
        result_dispatcher.cond.notify_all();
        drop(inner);
        drop(clones);
        gc::unregister_this_thread();
    });

    Arc::into_raw(result)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    extern "C" fn double_thunk(arg: *mut c_void) -> *mut c_void {
        let n = arg as usize;
        (n * 2) as *mut c_void
    }

    #[test]
    fn spawn_await_roundtrip() {
        tyra_rt_init();
        let task = unsafe { tyra_task_spawn(double_thunk, 21 as *mut c_void) };
        let result = unsafe { tyra_task_await(task) } as usize;
        assert_eq!(result, 42);
    }

    #[test]
    fn join_all_many() {
        tyra_rt_init();
        let n = 100i64;
        let tasks: Vec<*const Task> = (0..n)
            .map(|i| unsafe { tyra_task_spawn(double_thunk, i as *mut c_void) })
            .collect();
        let mut results = vec![ptr::null_mut::<c_void>(); n as usize];
        unsafe {
            tyra_task_join_all(tasks.as_ptr(), n, results.as_mut_ptr());
        }
        for (i, r) in results.iter().enumerate() {
            assert_eq!(*r as usize, i * 2);
        }
    }

    // Shared counter passed via the thunk arg so this test does not race
    // against any other test using a global.
    extern "C" fn bump_thunk(arg: *mut c_void) -> *mut c_void {
        let counter = arg as *const AtomicUsize;
        unsafe { &*counter }.fetch_add(1, Ordering::SeqCst);
        ptr::null_mut()
    }

    #[test]
    fn concurrent_increments() {
        tyra_rt_init();
        let counter = Arc::new(AtomicUsize::new(0));
        let n = 1000i64;
        let arg = Arc::as_ptr(&counter) as *mut c_void;
        let tasks: Vec<*const Task> = (0..n)
            .map(|_| unsafe { tyra_task_spawn(bump_thunk, arg) })
            .collect();
        let mut results = vec![ptr::null_mut::<c_void>(); n as usize];
        unsafe {
            tyra_task_join_all(tasks.as_ptr(), n, results.as_mut_ptr());
        }
        assert_eq!(counter.load(Ordering::SeqCst), n as usize);
    }

    extern "C" fn sleepy_thunk(arg: *mut c_void) -> *mut c_void {
        let millis = arg as u64;
        std::thread::sleep(std::time::Duration::from_millis(millis));
        (millis * 10) as *mut c_void
    }

    #[test]
    fn select_returns_first_completion() {
        tyra_rt_init();
        // Three tasks sleeping 100ms, 500ms, 1000ms. Select must resolve
        // with the 100ms task's result (1000).
        let slow = unsafe { tyra_task_spawn(sleepy_thunk, 1000 as *mut c_void) };
        let mid = unsafe { tyra_task_spawn(sleepy_thunk, 500 as *mut c_void) };
        let fast = unsafe { tyra_task_spawn(sleepy_thunk, 100 as *mut c_void) };
        let handles = [slow, mid, fast];
        let sel = unsafe { tyra_task_select(handles.as_ptr(), handles.len() as i64) };
        let result = unsafe { tyra_task_await(sel) } as usize;
        assert_eq!(result, 1000, "select should surface the fastest task");
        // Drain the other handles so Arc refcounts hit zero cleanly.
        for h in [slow, mid, fast] {
            let _ = unsafe { tyra_task_await(h) };
        }
    }

    /// Exercise the early-exit path where a source task is already Done
    /// at the moment `tyra_task_select` is called. Deterministic: we
    /// drive the source's state to Done by first spawning it, cloning
    /// its Arc, and waiting for the worker to transition the state
    /// (via Condvar) — no wall-clock sleep.
    #[test]
    fn select_honours_already_done() {
        tyra_rt_init();
        // Spawn a task. Clone its Arc so we can both peek at state and
        // still pass the raw handle into select.
        let raw = unsafe { tyra_task_spawn(sleepy_thunk, 0 as *mut c_void) };
        let arc: Arc<Task> = unsafe {
            Arc::increment_strong_count(raw);
            Arc::from_raw(raw)
        };
        // Block the test thread on Condvar until the worker publishes Done.
        {
            let mut inner = arc.inner.lock().unwrap();
            while matches!(inner.state, TaskState::Pending) {
                inner = arc.cond.wait(inner).unwrap();
            }
        }
        drop(arc);
        // Now the worker has transitioned. Call select; it should take
        // the early-exit path on the first iteration.
        let handles = [raw];
        let sel = unsafe { tyra_task_select(handles.as_ptr(), handles.len() as i64) };
        let result = unsafe { tyra_task_await(sel) } as usize;
        assert_eq!(result, 0);
        let _ = unsafe { tyra_task_await(raw) };
    }
}
