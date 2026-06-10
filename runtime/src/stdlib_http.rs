//! HTTP client stdlib backing (§17.3.3 candidate, Tier 2). M11 phase 1.
//!
//! Exposes `tyra_http_get` + accessors as C ABI intrinsics for
//! `stdlib/http/client.ty`. Responses are buffered fully into a heap
//! struct and handed to Tyra as an opaque `i64` handle (0 = error).
//!
//! Transport: the `ureq` crate with rustls TLS. ureq is a blocking
//! client, which fits Tyra's model: HTTP calls run on worker threads
//! when spawned, and synchronous calls on the main thread are fine for
//! CLI / one-shot tools. Picking ureq over libcurl FFI keeps the runtime
//! free of `-lssl` / `-lcurl` system dependencies.
//!
//! Error model: ALL HTTP responses (2xx, 4xx, 5xx) are surfaced as Ok
//! (callers inspect `status()`). Only transport failures (DNS, refused,
//! TLS handshake, timeout) become `FetchError`. Errno:
//!   0 = Ok
//!   1 = Timeout
//!   2 = NetworkError (catch-all; message via `tyra_http_errmsg`)
//!
//! v0.1 limitations:
//! - GET only (POST / headers / body / auth defer to M11 phase 1b+).
//! - Response is leaked via `Box::leak` (same policy as json). Each
//!   successful request leaks one `HttpResponse` for process lifetime.
//! - Default 30s read/connect timeout, no user control yet.

use std::cell::Cell;
use std::ffi::{CStr, CString};
use std::io;
use std::os::raw::{c_char, c_int};
use std::sync::OnceLock;
use std::time::Duration;

pub(crate) struct HttpResponse {
    status: i64,
    body: CString,
}

thread_local! {
    static HTTP_ERRNO: Cell<c_int> = const { Cell::new(0) };
    // Raw pointer to the most recent error message. Each `set_err` call
    // Box::leak's a fresh CString and stores its pointer here. Leaked
    // CStrings are never reclaimed, but the bound is "one leak per
    // transport error", which is acceptable for v0.1 CLI workloads and
    // avoids the alternatives: (a) per-read allocation (unbounded leak
    // per call, worse than per-error), (b) a RefCell whose contents can
    // be dropped while a prior `tyra_http_errmsg` pointer is still held
    // by Tyra as a String field (stale pointer in a stored FetchError).
    static HTTP_ERRMSG_PTR: Cell<*const c_char> = const { Cell::new(EMPTY_MSG.as_ptr() as *const c_char) };
}

static EMPTY_MSG: &[u8] = b"\0";

fn set_ok() {
    HTTP_ERRNO.with(|e| e.set(0));
    HTTP_ERRMSG_PTR.with(|p| p.set(EMPTY_MSG.as_ptr() as *const c_char));
}

fn set_err(code: c_int, message: impl Into<String>) {
    HTTP_ERRNO.with(|e| e.set(code));
    let msg = message.into();
    let cs = CString::new(msg).unwrap_or_else(|_| CString::new("network error").unwrap());
    // Leak once per error; the pointer remains valid for the rest of the
    // process so Tyra can safely stash it inside a FetchError payload
    // that outlives subsequent HTTP calls.
    let leaked: *const c_char = cs.into_raw() as *const c_char;
    HTTP_ERRMSG_PTR.with(|p| p.set(leaked));
}

/// True when a ureq error bottoms out in an `io::ErrorKind::TimedOut` or
/// `WouldBlock`. Walks the source chain because ureq wraps the I/O error
/// inside `ureq::Error::Transport` whose cause is the real `io::Error`.
/// Prefers this over display-string matching — the OS message text is not
/// portable across platforms.
fn looks_like_timeout(err: &ureq::Error) -> bool {
    let mut source: Option<&(dyn std::error::Error + 'static)> = Some(err);
    while let Some(e) = source {
        if let Some(io_err) = e.downcast_ref::<io::Error>() {
            return matches!(
                io_err.kind(),
                io::ErrorKind::TimedOut | io::ErrorKind::WouldBlock
            );
        }
        source = e.source();
    }
    false
}

/// Lazily-initialized agent so repeated GETs reuse connection keep-alive
/// and TLS session state. Built the first time an HTTP call runs; the
/// 30s timeout covers the entire request round trip (connect + read).
fn agent() -> &'static ureq::Agent {
    static AGENT: OnceLock<ureq::Agent> = OnceLock::new();
    AGENT.get_or_init(|| {
        ureq::AgentBuilder::new()
            .timeout(Duration::from_secs(30))
            .build()
    })
}

/// GET `url`; returns an opaque handle on success, 0 on transport error.
/// Any HTTP status (2xx/4xx/5xx) counts as success — callers inspect
/// `tyra_http_status` / `tyra_http_body`.
///
/// # Safety
/// `url` must be a null-terminated UTF-8 string.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tyra_http_get(url: *const c_char) -> i64 {
    if url.is_null() {
        set_err(2, "null url");
        return 0;
    }
    let url_str = match unsafe { CStr::from_ptr(url) }.to_str() {
        Ok(s) => s,
        Err(_) => {
            set_err(2, "invalid utf-8 in url");
            return 0;
        }
    };

    let response = match agent().get(url_str).call() {
        Ok(r) => r,
        Err(ureq::Error::Status(code, r)) => {
            // 4xx/5xx — promote to Ok with the real status so Tyra callers
            // can inspect the body regardless of the HTTP failure class.
            let body = read_body_bounded(r);
            return leak_response(code as i64, body);
        }
        Err(e) => {
            let msg = format!("{e}");
            if looks_like_timeout(&e) {
                set_err(1, msg);
            } else {
                // All other transport errors collapse into NetworkError
                // (2). ureq's ErrorKind is #[non_exhaustive] and expands
                // across versions, so we deliberately do not branch on it.
                set_err(2, msg);
            }
            return 0;
        }
    };
    let status = response.status() as i64;
    let body = read_body_bounded(response);
    leak_response(status, body)
}

/// Upper bound on a single response body we buffer into the leaked
/// handle. Keeps the per-request leak bounded for long-running callers
/// (see module doc). v0.1 does not expose streaming; oversized bodies
/// are truncated at this limit and a trailing newline is not added.
const MAX_BODY_BYTES: u64 = 10 * 1024 * 1024;

fn read_body_bounded(r: ureq::Response) -> String {
    use std::io::Read;
    let mut buf = String::new();
    let _ = r
        .into_reader()
        .take(MAX_BODY_BYTES)
        .read_to_string(&mut buf);
    buf
}

fn leak_response(status: i64, body: String) -> i64 {
    set_ok();
    // Binary payloads containing NUL bytes cannot round-trip through Tyra's
    // C-string-based String type. v0.1 truncates at the first NUL to keep
    // the ABI simple; the status is still accurate so callers can detect
    // the mismatch (e.g. by checking Content-Type before reading body).
    // Revisit when Tyra grows a `Bytes` type distinct from `String`.
    let cleaned: String = match CString::new(body.clone()) {
        Ok(_) => body,
        Err(nul_err) => {
            let pos = nul_err.nul_position();
            body[..pos].to_string()
        }
    };
    let body_cs = CString::new(cleaned).unwrap_or_else(|_| CString::new("").unwrap());
    let boxed = Box::new(HttpResponse {
        status,
        body: body_cs,
    });
    Box::leak(boxed) as *const HttpResponse as i64
}

unsafe fn resp_ref<'a>(h: i64) -> Option<&'a HttpResponse> {
    if h == 0 {
        None
    } else {
        Some(unsafe { &*(h as *const HttpResponse) })
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn tyra_http_status(h: i64) -> i64 {
    unsafe { resp_ref(h) }.map(|r| r.status).unwrap_or(0)
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn tyra_http_body(h: i64) -> *const c_char {
    unsafe { resp_ref(h) }
        .map(|r| r.body.as_ptr())
        .unwrap_or(c"".as_ptr())
}

#[unsafe(no_mangle)]
pub extern "C" fn tyra_http_errno() -> c_int {
    HTTP_ERRNO.with(|e| e.get())
}

/// Return the last transport error message as a permanent pointer
/// (leaked per error — see `set_err`). Safe for Tyra callers to store
/// inside a FetchError payload and carry across subsequent HTTP calls;
/// no per-read allocation. Repeated calls between errors return the
/// same pointer cheaply.
#[unsafe(no_mangle)]
pub extern "C" fn tyra_http_errmsg() -> *const c_char {
    HTTP_ERRMSG_PTR.with(|p| p.get())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cstr(s: &str) -> CString {
        CString::new(s).unwrap()
    }

    #[test]
    fn null_url_reports_network_error() {
        let h = unsafe { tyra_http_get(std::ptr::null()) };
        assert_eq!(h, 0);
        assert_eq!(tyra_http_errno(), 2);
        let msg = tyra_http_errmsg();
        let s = unsafe { CStr::from_ptr(msg) }.to_str().unwrap().to_string();
        assert!(s.contains("null"));
    }

    #[test]
    fn invalid_scheme_is_network_error() {
        let url = cstr("not-a-url");
        let h = unsafe { tyra_http_get(url.as_ptr()) };
        assert_eq!(h, 0);
        assert_eq!(tyra_http_errno(), 2);
        let msg = tyra_http_errmsg();
        let _ = unsafe { CStr::from_ptr(msg) }.to_str().unwrap().to_string();
    }

    #[test]
    fn errmsg_no_alloc_on_repeated_calls() {
        // Regression guard: `tyra_http_errmsg` used to allocate + leak one
        // CString per call. It now returns a borrowed pointer into the
        // thread-local buffer. Calling it many times must be cheap.
        let url = cstr("not-a-url");
        let _ = unsafe { tyra_http_get(url.as_ptr()) };
        let p1 = tyra_http_errmsg();
        let p2 = tyra_http_errmsg();
        // Pointer is stable across calls that do not mutate the buffer.
        assert_eq!(p1, p2);
    }

    // Live-network tests are opt-in: they rely on external services and
    // will flake in offline CI. Run with `TYRA_HTTP_LIVE=1 cargo test`.
    #[test]
    fn live_get_smoke() {
        if std::env::var("TYRA_HTTP_LIVE").is_err() {
            return;
        }
        let url = cstr("https://example.com/");
        let h = unsafe { tyra_http_get(url.as_ptr()) };
        assert_ne!(
            h,
            0,
            "live GET to example.com failed: errno={}",
            tyra_http_errno()
        );
        assert_eq!(unsafe { tyra_http_status(h) }, 200);
        let body_ptr = unsafe { tyra_http_body(h) };
        let body = unsafe { CStr::from_ptr(body_ptr) }.to_str().unwrap();
        assert!(body.contains("Example"), "body didn't contain sentinel");
    }
}
