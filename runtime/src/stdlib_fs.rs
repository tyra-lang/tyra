//! File system stdlib backing (§22, Tier 2). M10 phase 1.
//!
//! Exposes `tyra_fs_read` / `tyra_fs_errno` to Tyra. The Tyra-side wrapper
//! in `stdlib/fs.tyra` turns these into `Result<String, FsError>`.
//!
//! Errno codes (matched by stdlib/fs.tyra):
//!   0 = Ok
//!   1 = NotFound
//!   2 = PermissionDenied
//!   3 = IoError (catch-all)
//!
//! v0.1 limitation: returned strings are allocated via `CString::into_raw`
//! (system allocator). Boehm GC scans conservatively so the pointer stays
//! reachable while referenced, but the buffer is never freed. Acceptable
//! for v0.1 CLI workloads; revisit when GC_malloc is wired through the
//! runtime (M9 Known follow-up).
//!
//! Thread-safety contract: `tyra_fs_read` followed by `tyra_fs_errno` must
//! occur on the same OS thread with no `.await` point (= no scheduler
//! handoff in M9) in between. `stdlib/fs.tyra::read_to_string` satisfies
//! this by calling both intrinsics back-to-back. Ad-hoc user code that
//! interleaves `spawn`/`.await` between the two will lose the errno.

use std::cell::Cell;
use std::ffi::{CStr, CString};
use std::fs;
use std::io::ErrorKind;
use std::os::raw::{c_char, c_int};

thread_local! {
    static FS_ERRNO: Cell<c_int> = const { Cell::new(0) };
}

fn set_errno(code: c_int) {
    FS_ERRNO.with(|e| e.set(code));
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
        set_errno(3);
        return EMPTY.as_ptr() as *const c_char;
    }
    let path_str = match unsafe { CStr::from_ptr(path) }.to_str() {
        Ok(s) => s,
        Err(_) => {
            set_errno(3);
            return EMPTY.as_ptr() as *const c_char;
        }
    };
    match fs::read_to_string(path_str) {
        Ok(content) => {
            set_errno(0);
            match CString::new(content) {
                Ok(cs) => cs.into_raw() as *const c_char,
                Err(_) => {
                    // File contains an interior NUL. Treat as IoError.
                    set_errno(3);
                    EMPTY.as_ptr() as *const c_char
                }
            }
        }
        Err(e) => {
            set_errno(map_io_error(e.kind()));
            EMPTY.as_ptr() as *const c_char
        }
    }
}

/// Return the most recent fs errno for the calling thread.
#[unsafe(no_mangle)]
pub extern "C" fn tyra_fs_errno() -> c_int {
    FS_ERRNO.with(|e| e.get())
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
    }

    fn tempfile_path(name: &str) -> std::path::PathBuf {
        let mut p = std::env::temp_dir();
        p.push(format!("{name}-{}", std::process::id()));
        p
    }
}
