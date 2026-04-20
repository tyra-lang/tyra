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
//! Boehm GC integration is gated by the `libgc` Cargo feature. When enabled
//! (the staticlib build used for Tyra binaries), worker threads register
//! with the collector via `GC_register_my_thread` so their stacks are
//! included in conservative scans. Cargo-test builds leave the feature off
//! to avoid requiring libgc at test link time.

use crossbeam_channel::{Receiver, Sender, unbounded};
use std::os::raw::c_void;
use std::ptr;
use std::sync::{Arc, Condvar, Mutex, OnceLock};
use std::thread;

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

/// Thunk signature emitted by codegen for each `spawn` site.
/// Takes opaque arg pointer, returns opaque result pointer.
pub type ThunkFn = unsafe extern "C" fn(*mut c_void) -> *mut c_void;

/// A task handle. Opaque to codegen; lifetime managed via `Arc`.
pub struct Task {
    mutex: Mutex<TaskState>,
    cond: Condvar,
}

enum TaskState {
    Pending,
    Done(*mut c_void),
}

// SAFETY: `Done(*mut c_void)` holds a pointer produced by a user thunk. We
// move it from worker to awaiter exactly once, under the mutex.
unsafe impl Send for TaskState {}
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
        let mut state = job.task.mutex.lock().unwrap();
        *state = TaskState::Done(result);
        job.task.cond.notify_all();
        // `state` drops here, releasing the mutex. `job` drops at end of
        // loop body, decrementing the Arc refcount. The awaiter's Arc keeps
        // the Task alive long enough for it to observe `Done`.
        drop(state);
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
// Boehm GC thread registration (feature-gated).
// ---------------------------------------------------------------------------

#[cfg(feature = "libgc")]
mod gc {
    use std::os::raw::c_void;

    #[repr(C)]
    struct GCStackBase {
        mem_base: *mut c_void,
        reg_base: *mut c_void,
    }

    unsafe extern "C" {
        fn GC_get_stack_base(sb: *mut GCStackBase) -> i32;
        fn GC_register_my_thread(sb: *const GCStackBase) -> i32;
        fn GC_unregister_my_thread() -> i32;
    }

    pub(super) fn register_this_thread() {
        unsafe {
            let mut sb = GCStackBase {
                mem_base: std::ptr::null_mut(),
                reg_base: std::ptr::null_mut(),
            };
            if GC_get_stack_base(&mut sb) == 0 {
                GC_register_my_thread(&sb);
            }
        }
    }

    pub(super) fn unregister_this_thread() {
        unsafe {
            GC_unregister_my_thread();
        }
    }
}

#[cfg(not(feature = "libgc"))]
mod gc {
    pub(super) fn register_this_thread() {}
    pub(super) fn unregister_this_thread() {}
}

// ---------------------------------------------------------------------------
// C ABI
// ---------------------------------------------------------------------------

/// Initialize the scheduler. Idempotent; second call is a no-op.
/// Must be invoked from `main` before any `tyra_task_spawn`.
#[unsafe(no_mangle)]
pub extern "C" fn tyra_rt_init() {
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
/// # Safety
/// `thunk` must be a valid function pointer; `arg` must be valid for the
/// lifetime of the task (typically a GC-managed argument struct).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tyra_task_spawn(thunk: ThunkFn, arg: *mut c_void) -> *const Task {
    let task = Arc::new(Task {
        mutex: Mutex::new(TaskState::Pending),
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
    let mut state = task.mutex.lock().unwrap();
    loop {
        match *state {
            TaskState::Done(result) => return result,
            TaskState::Pending => {
                state = task.cond.wait(state).unwrap();
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

}
