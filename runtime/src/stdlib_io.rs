//! stdin stdlib backing.
//!
//! Exposes `tyra_io_read_line` / `tyra_io_read_to_end` / `tyra_io_eof` to
//! Tyra. The Tyra-side wrapper in `stdlib/io.tyra` turns these into
//! `Option<String>` / `String` / `Bool`.
//!
//! v0.1 semantics:
//! - `tyra_io_read_line` reads one LF-terminated line from stdin, strips
//!   the trailing '\n' (and '\r' before it, if any), and returns a freshly
//!   allocated C string. On EOF with no remaining bytes, returns the empty
//!   string `""`; callers distinguish EOF by calling `tyra_io_eof`
//!   immediately afterward, which returns 1 iff the previous call reached
//!   EOF without reading any characters.
//! - `tyra_io_read_to_end` consumes all remaining stdin as UTF-8 and
//!   returns it as a C string. Always returns a valid pointer, even if
//!   stdin is empty (empty string).
//! - `tyra_io_eof` returns the EOF state set by the most recent
//!   `tyra_io_read_line` call on the same thread. 1 = EOF, 0 = not EOF.
//!
//! Allocation: `CString::into_raw` allocates via Rust's system allocator,
//! not `GC_malloc`. The Boehm GC does not manage these buffers, so they
//! are never reclaimed in v0.1 — same trade-off as `tyra_fs_read`.
//! Acceptable for CLI / one-shot tools; long-lived processes that loop on
//! `read_line` leak heap memory unboundedly until GC_malloc integration lands.
//!
//! NUL handling: input containing interior NUL bytes is truncated at the
//! first NUL (Tyra `String` is C-string-backed in v0.1). Binary stdin is
//! not a supported workload.

use std::cell::Cell;
use std::ffi::CString;
use std::io::{self, BufRead, Read};
use std::os::raw::{c_char, c_int};

thread_local! {
    static IO_EOF: Cell<c_int> = const { Cell::new(0) };
}

fn set_eof(eof: bool) {
    IO_EOF.with(|e| e.set(if eof { 1 } else { 0 }));
}

fn leak_cstring(s: String) -> *const c_char {
    // Strip interior NULs to avoid CString::new failing on binary input.
    let mut cleaned = s;
    if let Some(pos) = cleaned.as_bytes().iter().position(|&b| b == 0) {
        cleaned.truncate(pos);
    }
    match CString::new(cleaned) {
        Ok(c) => c.into_raw(),
        Err(_) => CString::new("").unwrap().into_raw(),
    }
}

/// Read one line from stdin (without the trailing newline). Returns the
/// empty string on EOF; callers must check `tyra_io_eof` to distinguish
/// a genuine empty line from stdin closure.
#[unsafe(no_mangle)]
pub extern "C" fn tyra_io_read_line() -> *const c_char {
    let stdin = io::stdin();
    let mut lock = stdin.lock();
    let mut buf = String::new();
    match lock.read_line(&mut buf) {
        Ok(0) => {
            set_eof(true);
            leak_cstring(String::new())
        }
        Ok(_) => {
            set_eof(false);
            // Strip the LF (and CR before it, if present).
            if buf.ends_with('\n') {
                buf.pop();
                if buf.ends_with('\r') {
                    buf.pop();
                }
            }
            leak_cstring(buf)
        }
        Err(_) => {
            // On read error treat as EOF-with-empty-result for v0.1.
            set_eof(true);
            leak_cstring(String::new())
        }
    }
}

/// Read all remaining stdin as UTF-8. Empty string if stdin is empty.
#[unsafe(no_mangle)]
pub extern "C" fn tyra_io_read_to_end() -> *const c_char {
    let stdin = io::stdin();
    let mut lock = stdin.lock();
    let mut buf = String::new();
    let _ = lock.read_to_string(&mut buf);
    set_eof(true);
    leak_cstring(buf)
}

/// Return 1 iff the most recent `tyra_io_read_line` on this thread hit
/// end-of-file. Resets are explicit via the next read_line call.
#[unsafe(no_mangle)]
pub extern "C" fn tyra_io_eof() -> c_int {
    IO_EOF.with(|e| e.get())
}
