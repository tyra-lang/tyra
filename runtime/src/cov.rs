// Coverage counter runtime (ADR 0014 §coverage).
//
// `tyra_cov_init` is called once from main with a pointer to the counter
// array embedded in the binary and the counter count.  It registers an
// atexit handler that writes `$TYRA_COV_DIR/<pid>.covraw` on process exit.
//
// File format: little-endian i64 per counter, no header.  The companion
// `<binary>.tyra-covmap` sidecar (written by the compiler) maps each index
// to a (file_id, line) source location.

use std::sync::atomic::{AtomicI64, AtomicPtr, Ordering};

static TYRA_COV_COUNTERS: AtomicPtr<i64> = AtomicPtr::new(core::ptr::null_mut());
static TYRA_COV_N: AtomicI64 = AtomicI64::new(0);

unsafe extern "C" {
    fn atexit(f: unsafe extern "C" fn()) -> i32;
}

unsafe extern "C" fn tyra_cov_write_raw() {
    let n = TYRA_COV_N.load(Ordering::Relaxed) as usize;
    let ptr = TYRA_COV_COUNTERS.load(Ordering::Relaxed);
    if ptr.is_null() || n == 0 {
        return;
    }
    let dir = match std::env::var("TYRA_COV_DIR") {
        Ok(d) => d,
        Err(_) => return,
    };
    // SAFETY: ptr was set by tyra_cov_init from a valid LLVM global array
    // of exactly n elements. The array lives for the duration of the process.
    let counters = unsafe { std::slice::from_raw_parts(ptr, n) };
    let pid = std::process::id();
    let path = format!("{dir}/{pid}.covraw");
    let mut bytes: Vec<u8> = Vec::with_capacity(n * 8);
    for &c in counters {
        bytes.extend_from_slice(&c.to_le_bytes());
    }
    let _ = std::fs::write(&path, &bytes);
}

/// Called once from the Tyra main entry when `--coverage` is active.
/// `counters` points to the `@.tyra_counters` LLVM global array; `n` is its
/// element count.  Registers an atexit handler to flush counts to disk.
#[unsafe(no_mangle)]
pub extern "C" fn tyra_cov_init(counters: *mut i64, n: i64) {
    TYRA_COV_COUNTERS.store(counters, Ordering::SeqCst);
    TYRA_COV_N.store(n, Ordering::SeqCst);
    // atexit handlers run on exit() (including exit(101)/exit(102)) but NOT
    // on abort() or SIGKILL — those cases lose coverage data (best-effort).
    unsafe {
        atexit(tyra_cov_write_raw);
    }
}
