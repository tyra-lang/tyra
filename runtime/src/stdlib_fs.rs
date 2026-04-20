//! File system stdlib backing (§17.3.1, Tier 2). M10 phase 1 + errmsg.
//!
//! Exposes `tyra_fs_read` / `tyra_fs_errno` / `tyra_fs_errmsg` /
//! `tyra_fs_write` / `tyra_fs_exists` to Tyra. The Tyra-side wrapper in
//! `stdlib/fs.tyra` turns these into `Result<_, FsError>` / `Bool`.
//!
//! Errno codes (matched by stdlib/fs.tyra):
//!   0 = Ok
//!   1 = NotFound
//!   2 = PermissionDenied
//!   3 = IoError (catch-all; `tyra_fs_errmsg` carries a description)
//!
//! v0.1 limitations:
//! - Returned strings are allocated via `CString::into_raw` (system
//!   allocator). Boehm GC scans conservatively so the pointer stays
//!   reachable while referenced, but the buffer is never freed. Acceptable
//!   for v0.1 CLI workloads; revisit when GC_malloc is wired through the
//!   runtime (M9 follow-up).
//! - The `__fs_*` intrinsics are registered in the Tyra prelude so that
//!   `stdlib/fs.tyra` can call them without an `import` (which would
//!   create a module cycle). Direct user calls are technically possible
//!   but unsupported — the thread-local errno/errmsg contract assumes
//!   the stdlib wrapper pattern and user code interleaving multiple fs
//!   ops without inspecting errno immediately will lose state.
//!
//! Thread-safety contract: `tyra_fs_read` followed by `tyra_fs_errno` /
//! `tyra_fs_errmsg` must occur on the same OS thread with no `.await`
//! point (= no scheduler handoff in M9) in between. `stdlib/fs.tyra`
//! satisfies this by calling the intrinsics back-to-back.

use std::cell::{Cell, RefCell};
use std::ffi::{CStr, CString};
use std::fs;
use std::io;
use std::io::ErrorKind;
use std::os::raw::{c_char, c_int};

thread_local! {
    static FS_ERRNO: Cell<c_int> = const { Cell::new(0) };
    // Last I/O error message for the catch-all `IoError` errno (3).
    // Invariant: every fs entrypoint either calls `set_errno` (which
    // clears the message) or `set_io_error*` (which sets it). The message
    // is only meaningful when `FS_ERRNO == 3`; for other codes it is "".
    // `tyra_fs_errmsg` copies into a caller-owned CString so the
    // thread-local can be overwritten safely afterwards.
    static FS_ERRMSG: RefCell<String> = const { RefCell::new(String::new()) };
}

/// Set errno and unconditionally clear the stored error message. Callers
/// that want to carry a message through alongside errno=3 must use
/// `set_io_error` or `set_synthetic_io_error` instead.
fn set_errno(code: c_int) {
    FS_ERRNO.with(|e| e.set(code));
    FS_ERRMSG.with(|m| m.borrow_mut().clear());
}

/// Record an `io::Error` from the OS: set errno per its kind and, when
/// the result falls into the `IoError` catch-all (3), surface the
/// underlying description via `to_string()`.
fn set_io_error(err: &io::Error) {
    let code = map_io_error(err.kind());
    FS_ERRNO.with(|e| e.set(code));
    if code == 3 {
        FS_ERRMSG.with(|m| *m.borrow_mut() = err.to_string());
    } else {
        FS_ERRMSG.with(|m| m.borrow_mut().clear());
    }
}

/// Record a synthetic IoError (errno=3) for failure modes that do not
/// originate from an `io::Error` — e.g. null path, invalid UTF-8 in the
/// path, or interior NUL in file contents. The message is stored
/// verbatim so Tyra callers get a descriptive `FsError::IoError(message)`.
fn set_synthetic_io_error(message: &str) {
    FS_ERRNO.with(|e| e.set(3));
    FS_ERRMSG.with(|m| *m.borrow_mut() = message.to_string());
}

fn map_io_error(kind: ErrorKind) -> c_int {
    match kind {
        ErrorKind::NotFound => 1,
        ErrorKind::PermissionDenied => 2,
        _ => 3,
    }
}

/// Read a UTF-8 file to a newly-allocated C string.
///
/// On success: returns a non-null `*const c_char` and sets errno to 0.
/// On failure: returns a pointer to a static empty string and sets errno.
///
/// # Safety
/// `path` must be a null-terminated UTF-8 string.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tyra_fs_read(path: *const c_char) -> *const c_char {
    static EMPTY: &[u8] = b"\0";
    if path.is_null() {
        set_synthetic_io_error("null path");
        return EMPTY.as_ptr() as *const c_char;
    }
    let path_str = match unsafe { CStr::from_ptr(path) }.to_str() {
        Ok(s) => s,
        Err(_) => {
            set_synthetic_io_error("invalid utf-8 in path");
            return EMPTY.as_ptr() as *const c_char;
        }
    };
    match fs::read_to_string(path_str) {
        Ok(content) => {
            set_errno(0);
            match CString::new(content) {
                Ok(cs) => cs.into_raw() as *const c_char,
                Err(_) => {
                    set_synthetic_io_error("file contains interior NUL byte");
                    EMPTY.as_ptr() as *const c_char
                }
            }
        }
        Err(e) => {
            set_io_error(&e);
            EMPTY.as_ptr() as *const c_char
        }
    }
}

/// Return the most recent fs errno for the calling thread.
#[unsafe(no_mangle)]
pub extern "C" fn tyra_fs_errno() -> c_int {
    FS_ERRNO.with(|e| e.get())
}

/// Write `contents` to `path`, creating or truncating the file.
///
/// Sets errno to 0 on success or 1/2/3 on failure (same mapping as
/// `tyra_fs_read`). Returns void — callers read the errno separately,
/// matching the read-side contract.
///
/// # Safety
/// Both pointers must be null-terminated UTF-8 strings.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tyra_fs_write(path: *const c_char, contents: *const c_char) {
    if path.is_null() || contents.is_null() {
        set_synthetic_io_error("null path or contents");
        return;
    }
    let path_str = match unsafe { CStr::from_ptr(path) }.to_str() {
        Ok(s) => s,
        Err(_) => {
            set_synthetic_io_error("invalid utf-8 in path");
            return;
        }
    };
    let bytes = unsafe { CStr::from_ptr(contents) }.to_bytes();
    match fs::write(path_str, bytes) {
        Ok(()) => set_errno(0),
        Err(e) => set_io_error(&e),
    }
}

/// Return the last I/O error message for the calling thread as a caller-
/// owned heap CString. Meaningful only when `tyra_fs_errno() == 3`
/// (IoError catch-all); returns an empty string for other codes.
///
/// The returned pointer is heap-allocated via `CString::into_raw` and will
/// outlive any subsequent fs call. v0.1 accepts the leak (same trade-off
/// as `tyra_json_err_msg`). Note: even the empty-string case allocates.
/// `stdlib/fs.tyra` calls this only under the `IoError` match arm, so in
/// normal Tyra code paths the leak occurs at most once per failed op.
#[unsafe(no_mangle)]
pub extern "C" fn tyra_fs_errmsg() -> *const c_char {
    let s = FS_ERRMSG.with(|m| m.borrow().clone());
    let cs = CString::new(s).unwrap_or_else(|_| CString::new("io error").unwrap());
    cs.into_raw() as *const c_char
}

/// Return 1 if `path` refers to an existing filesystem entry, 0 otherwise.
///
/// Does not distinguish file vs directory, nor does it touch errno — callers
/// wanting diagnostic detail should use `tyra_fs_read` and inspect errno.
///
/// # Safety
/// `path` must be a null-terminated UTF-8 string.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tyra_fs_exists(path: *const c_char) -> c_int {
    if path.is_null() {
        return 0;
    }
    let path_str = match unsafe { CStr::from_ptr(path) }.to_str() {
        Ok(s) => s,
        Err(_) => return 0,
    };
    if std::path::Path::new(path_str).exists() { 1 } else { 0 }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::CString;
    use std::io::Write;

    fn cstr(s: &str) -> CString {
        CString::new(s).unwrap()
    }

    #[test]
    fn read_existing_file() {
        let tmp = tempfile_path("tyra_fs_read_ok");
        {
            let mut f = std::fs::File::create(&tmp).unwrap();
            f.write_all(b"hello tyra").unwrap();
        }
        let path = cstr(tmp.to_str().unwrap());
        let ptr = unsafe { tyra_fs_read(path.as_ptr()) };
        assert_eq!(tyra_fs_errno(), 0);
        let got = unsafe { CStr::from_ptr(ptr) }.to_str().unwrap();
        assert_eq!(got, "hello tyra");
        let _ = std::fs::remove_file(&tmp);
    }

    #[test]
    fn read_missing_file_sets_not_found() {
        let path = cstr("/definitely/does/not/exist/tyra-fs-test");
        let _ = unsafe { tyra_fs_read(path.as_ptr()) };
        assert_eq!(tyra_fs_errno(), 1);
        // NotFound is not IoError; errmsg must be empty.
        let msg = tyra_fs_errmsg();
        let s = unsafe { CStr::from_ptr(msg) }.to_str().unwrap().to_string();
        assert_eq!(s, "");
        unsafe { drop(CString::from_raw(msg as *mut c_char)); }
    }

    #[cfg(unix)]
    #[test]
    fn io_catch_all_populates_errmsg() {
        // Reading `/` fails with ErrorKind::IsADirectory (Linux) or
        // ErrorKind::Other (older macOS), both mapping to errno 3.
        let path = cstr("/");
        let _ = unsafe { tyra_fs_read(path.as_ptr()) };
        assert_eq!(tyra_fs_errno(), 3);
        let msg = tyra_fs_errmsg();
        let s = unsafe { CStr::from_ptr(msg) }.to_str().unwrap().to_string();
        assert!(!s.is_empty(), "errmsg must be populated for IoError");
        unsafe { drop(CString::from_raw(msg as *mut c_char)); }
    }

    #[test]
    fn synthetic_io_error_populates_errmsg() {
        // Null-path failure goes through set_synthetic_io_error; errmsg
        // must be populated with a descriptive message, not a stale one.
        let _ = unsafe { tyra_fs_read(std::ptr::null()) };
        assert_eq!(tyra_fs_errno(), 3);
        let msg = tyra_fs_errmsg();
        let s = unsafe { CStr::from_ptr(msg) }.to_str().unwrap().to_string();
        assert!(s.contains("null"), "expected null-path message, got {s:?}");
        unsafe { drop(CString::from_raw(msg as *mut c_char)); }
    }

    #[test]
    fn errmsg_cleared_between_known_errno_and_iooerror() {
        // Regression guard for the pre-fix desync bug: a prior IoError
        // must not leak its message into a subsequent NotFound.
        let bad = cstr("/");
        let _ = unsafe { tyra_fs_read(bad.as_ptr()) };
        assert_eq!(tyra_fs_errno(), 3);
        let missing = cstr("/definitely/missing/tyra-test");
        let _ = unsafe { tyra_fs_read(missing.as_ptr()) };
        assert_eq!(tyra_fs_errno(), 1);
        let msg = tyra_fs_errmsg();
        let s = unsafe { CStr::from_ptr(msg) }.to_str().unwrap().to_string();
        assert_eq!(s, "", "NotFound must not carry prior IoError message");
        unsafe { drop(CString::from_raw(msg as *mut c_char)); }
    }

    #[test]
    fn write_then_read_roundtrip() {
        let tmp = tempfile_path("tyra_fs_write_ok");
        let path = cstr(tmp.to_str().unwrap());
        let body = cstr("written by tyra");
        unsafe { tyra_fs_write(path.as_ptr(), body.as_ptr()) };
        assert_eq!(tyra_fs_errno(), 0);
        let got = std::fs::read_to_string(&tmp).unwrap();
        assert_eq!(got, "written by tyra");
        let _ = std::fs::remove_file(&tmp);
    }

    #[test]
    fn exists_reports_presence() {
        let tmp = tempfile_path("tyra_fs_exists_ok");
        let path = cstr(tmp.to_str().unwrap());
        assert_eq!(unsafe { tyra_fs_exists(path.as_ptr()) }, 0);
        std::fs::write(&tmp, b"x").unwrap();
        assert_eq!(unsafe { tyra_fs_exists(path.as_ptr()) }, 1);
        let _ = std::fs::remove_file(&tmp);
    }

    fn tempfile_path(name: &str) -> std::path::PathBuf {
        let mut p = std::env::temp_dir();
        p.push(format!("{name}-{}", std::process::id()));
        p
    }
}
