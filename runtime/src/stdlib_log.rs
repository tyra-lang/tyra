//! Log stdlib backing (§17.3.x). v0.1 structured logging (stderr).
//!
//! Exposes `tyra_log_*` intrinsics consumed by `stdlib/log.tyra`.
//!
//! Scope (v0.1):
//!   info(msg: String)  -> Unit
//!   warn(msg: String)  -> Unit
//!   error(msg: String) -> Unit
//!
//! All levels write to stderr with a `[LEVEL] ` prefix and a trailing newline.

use std::ffi::CStr;
use std::os::raw::c_char;

fn write_log(level: &str, msg_ptr: *const c_char) {
    let msg = if msg_ptr.is_null() {
        "(null)"
    } else {
        unsafe { CStr::from_ptr(msg_ptr) }
            .to_str()
            .unwrap_or("(invalid utf-8)")
    };
    eprintln!("[{}] {}", level, msg);
}

/// `__log_info(msg: String) -> Unit`
#[unsafe(no_mangle)]
pub extern "C" fn tyra_log_info(msg: *const c_char) {
    write_log("INFO", msg);
}

/// `__log_warn(msg: String) -> Unit`
#[unsafe(no_mangle)]
pub extern "C" fn tyra_log_warn(msg: *const c_char) {
    write_log("WARN", msg);
}

/// `__log_error(msg: String) -> Unit`
#[unsafe(no_mangle)]
pub extern "C" fn tyra_log_error(msg: *const c_char) {
    write_log("ERROR", msg);
}
