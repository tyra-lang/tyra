//! HTTP server stdlib backing (§17.3.3 candidate, Tier 2). M11 phase 2.
//!
//! Minimal HTTP/1.1 server:
//! - Blocking single-threaded accept loop on 0.0.0.0:<port>.
//! - Exact-path routing (no wildcards / params in v0.1).
//! - Handler ABI: `fn(*const TyraRequest) -> *const TyraResponse` — plain
//!   Tyra data-struct pointers allocated via GC_malloc, matching the
//!   LLVM layout the codegen emits:
//!
//! ```text
//!   TyraRequest  = { *const c_char method, *const c_char path, *const c_char body }
//!   TyraResponse = { i64 status, *const c_char body }
//! ```
//! - Single-threaded: handlers run on the listener thread. OK for v0.1
//!   demo / dev-tool scope; upgrade to thread-per-connection or M9 spawn
//!   dispatch when performance matters.
//!
//! Out of scope for v0.1:
//!   - TLS (HTTPS) — ureq handles client-side; server needs rustls wiring.
//!   - HTTP/2, WebSocket, keep-alive reuse (connection is closed per req).
//!   - Header access, query-string parsing, cookies (body is captured raw).
//!   - Graceful shutdown, per-request timeouts, large-body streaming.

use std::collections::HashMap;
use std::ffi::CStr;
#[cfg(test)]
use std::ffi::CString;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpListener;
use std::os::raw::{c_char, c_int, c_void};
use std::sync::Mutex;

/// Function pointer matching `@handler(ptr) -> ptr` in LLVM.
type HandlerFn = unsafe extern "C" fn(req: *const TyraRequest) -> *const TyraResponse;

/// LLVM layout emitted by Tyra codegen for `data Request { method, path, body }`.
#[repr(C)]
#[doc(hidden)]
pub struct TyraRequest {
    method: *const c_char,
    path: *const c_char,
    body: *const c_char,
}

/// LLVM layout for `data Response { status: Int, body: String }`.
#[repr(C)]
#[doc(hidden)]
pub struct TyraResponse {
    status: i64,
    body: *const c_char,
}

/// Server handle. Exposed as `pub` so the `extern "C"` signatures that
/// carry it remain inside the `private_interfaces` lint limits; Tyra
/// consumers treat the handle as opaque (stored as Int in
/// `AppServer._handle`) and never construct or inspect `Server`
/// directly, so making this type crate-visible does not expand the
/// effective public API.
#[doc(hidden)]
pub struct Server {
    routes: Mutex<HashMap<(String, String), HandlerFn>>, // (method, path) -> handler
}

impl Server {
    fn new() -> Self {
        Server {
            routes: Mutex::new(HashMap::new()),
        }
    }
}

// SAFETY: HandlerFn is a raw C function pointer produced by Tyra codegen.
// Function pointers are Send/Sync in practice; we enforce synchronous
// single-threaded handler invocation in the accept loop regardless.
unsafe impl Send for Server {}
unsafe impl Sync for Server {}

unsafe extern "C" {
    fn GC_malloc(n: usize) -> *mut c_void;
}

/// Allocate a NUL-terminated copy of `s` in GC-managed memory so Tyra
/// code sees it as a normal String value (conservative GC scans will
/// keep it alive as long as the handler's arguments are reachable).
unsafe fn gc_cstring(s: &str) -> *const c_char {
    let len = s.len() + 1;
    let buf = unsafe { GC_malloc(len) } as *mut u8;
    unsafe {
        std::ptr::copy_nonoverlapping(s.as_ptr(), buf, s.len());
        *buf.add(s.len()) = 0;
    }
    buf as *const c_char
}

// ---------------------------------------------------------------------------
// C ABI
// ---------------------------------------------------------------------------

/// Create a new Server. Caller receives a leaked Arc-like handle; the
/// Server lives until process exit.
#[unsafe(no_mangle)]
pub extern "C" fn tyra_http_server_new() -> *const Server {
    Box::leak(Box::new(Server::new())) as *const Server
}

/// Register `handler` for `method path`. The handler function pointer
/// must have signature `fn(*const TyraRequest) -> *const TyraResponse`
/// — Tyra codegen guarantees this when the stdlib types the handler as
/// `fn(Request) -> Response`.
///
/// # Safety
/// `method` and `path` must be null-terminated UTF-8 strings.
/// `handler` must be a valid function pointer of the expected signature.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tyra_http_server_route(
    srv: *const Server,
    method: *const c_char,
    path: *const c_char,
    handler: HandlerFn,
) {
    assert!(!srv.is_null(), "tyra_http_server_route: null server");
    let srv = unsafe { &*srv };
    let m = unsafe { CStr::from_ptr(method) }
        .to_str()
        .unwrap_or("")
        .to_string();
    let p = unsafe { CStr::from_ptr(path) }
        .to_str()
        .unwrap_or("")
        .to_string();
    let mut routes = srv.routes.lock().unwrap();
    let key = (m, p);
    if routes.contains_key(&key) {
        // Route collision: the new handler wins (HashMap::insert).
        // Warn on stderr instead of panicking so a programmer error is
        // visible but doesn't hard-fail the process. Unconditional
        // `eprintln!` is a v0.1 compromise pending a `log` facade in
        // the runtime; replace with `log::warn!` once that lands. Users
        // who need silence can redirect stderr at process launch.
        eprintln!(
            "tyra_http_server_route: overwriting existing handler for {} {}",
            key.0, key.1
        );
    }
    routes.insert(key, handler);
}

/// Bind 0.0.0.0:port and accept connections until an unrecoverable
/// error. Returns 0 on orderly shutdown (currently unreachable — the
/// loop runs forever), 1 on bind error.
///
/// # Safety
/// `srv` must be a valid handle from `tyra_http_server_new`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tyra_http_server_listen(srv: *const Server, port: i64) -> c_int {
    assert!(!srv.is_null(), "tyra_http_server_listen: null server");
    let srv = unsafe { &*srv };
    let addr = format!("0.0.0.0:{port}");
    let listener = match TcpListener::bind(&addr) {
        Ok(l) => l,
        Err(_) => return 1,
    };
    for stream in listener.incoming() {
        let mut stream = match stream {
            Ok(s) => s,
            Err(_) => continue,
        };
        handle_connection(srv, &mut stream);
    }
    0
}

fn handle_connection(srv: &Server, stream: &mut std::net::TcpStream) {
    let (method, path, body) = match parse_request(stream) {
        Some(r) => r,
        None => {
            let _ = write_simple(stream, 400, "bad request");
            return;
        }
    };

    let routes = srv.routes.lock().unwrap();
    let handler = routes.get(&(method.clone(), path.clone())).copied();
    drop(routes);

    let handler = match handler {
        Some(h) => h,
        None => {
            let _ = write_simple(stream, 404, "not found");
            return;
        }
    };

    // Construct the Tyra Request struct. GC_malloc gives us a heap
    // allocation that is reachable for as long as the handler's input
    // parameter holds a reference to it.
    let req_box = unsafe {
        let p = GC_malloc(std::mem::size_of::<TyraRequest>()) as *mut TyraRequest;
        (*p).method = gc_cstring(&method);
        (*p).path = gc_cstring(&path);
        (*p).body = gc_cstring(&body);
        p as *const TyraRequest
    };

    // Call the Tyra handler. NOTE: Tyra's `panic()` lowers to `abort()`
    // (not Rust-style unwinding), so a Tyra-side panic will kill the
    // entire server process — there is no way for the runtime to
    // intercept it here. This limitation is documented on the stdlib
    // `listen` API and in example 04. If a future Tyra version adopts
    // unwinding panics, wrapping this call in `catch_unwind` will add
    // per-request isolation; today it would be dead code because
    // extern "C" boundaries with abort-on-panic semantics prevent the
    // unwind from being observable here.
    let resp_ptr = unsafe { handler(req_box) };
    if resp_ptr.is_null() {
        let _ = write_simple(stream, 500, "null response");
        return;
    }
    let resp = unsafe { &*resp_ptr };
    let body_str = if resp.body.is_null() {
        ""
    } else {
        unsafe { CStr::from_ptr(resp.body) }.to_str().unwrap_or("")
    };
    let _ = write_simple(stream, resp.status, body_str);
}

/// Parse `method path HTTP/1.1\r\n<headers>\r\n\r\n<body>`. Returns
/// (method, path-without-query, body). Query strings are discarded in
/// v0.1 (no Request.query accessor yet). Body capped at 1 MiB.
fn parse_request(stream: &mut std::net::TcpStream) -> Option<(String, String, String)> {
    let mut reader = BufReader::new(stream.try_clone().ok()?);
    let mut line = String::new();
    if reader.read_line(&mut line).ok()? == 0 {
        return None;
    }
    let request_line = line.trim_end_matches(&['\r', '\n'][..]).to_string();
    let mut parts = request_line.splitn(3, ' ');
    let method = parts.next()?.to_string();
    let path_full = parts.next()?.to_string();
    let path = path_full
        .split_once('?')
        .map(|(p, _)| p.to_string())
        .unwrap_or(path_full);
    // Parse headers to find Content-Length.
    let mut content_length: usize = 0;
    loop {
        let mut h = String::new();
        if reader.read_line(&mut h).ok()? == 0 {
            break;
        }
        let h = h.trim_end_matches(&['\r', '\n'][..]);
        if h.is_empty() {
            break;
        }
        if let Some((k, v)) = h.split_once(':')
            && k.trim().eq_ignore_ascii_case("content-length")
        {
            content_length = v.trim().parse().unwrap_or(0);
            if content_length > 1024 * 1024 {
                content_length = 1024 * 1024;
            }
        }
    }
    // Read body.
    let mut body = vec![0u8; content_length];
    if content_length > 0 {
        reader.read_exact(&mut body).ok()?;
    }
    let body_str = String::from_utf8_lossy(&body).into_owned();
    Some((method, path, body_str))
}

fn write_simple(stream: &mut std::net::TcpStream, status: i64, body: &str) -> std::io::Result<()> {
    let reason = match status {
        200 => "OK",
        201 => "Created",
        204 => "No Content",
        400 => "Bad Request",
        404 => "Not Found",
        500 => "Internal Server Error",
        _ => "OK",
    };
    let resp = format!(
        "HTTP/1.1 {status} {reason}\r\n\
         Content-Length: {}\r\n\
         Connection: close\r\n\
         \r\n{body}",
        body.len()
    );
    stream.write_all(resp.as_bytes())?;
    stream.flush()
}

/// Build a Tyra Response struct (layout { i64 status, ptr body }) from
/// Rust-side values. Exposed mainly for tests; Tyra handlers construct
/// Response themselves via normal `Response(status:, body:)` literals.
#[cfg(test)]
fn build_response(status: i64, body: &str) -> *const TyraResponse {
    unsafe {
        let p = GC_malloc(std::mem::size_of::<TyraResponse>()) as *mut TyraResponse;
        (*p).status = status;
        (*p).body = {
            let cs = CString::new(body).unwrap();
            cs.into_raw() as *const c_char
        };
        p as *const TyraResponse
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use std::net::TcpStream;
    use std::thread;

    extern "C" fn echo_handler(req: *const TyraRequest) -> *const TyraResponse {
        unsafe {
            let path = CStr::from_ptr((*req).path).to_str().unwrap_or("");
            let body = format!("echo {path}");
            build_response(200, &body)
        }
    }

    /// Start a server on an ephemeral port and return the bound port
    /// number once accept is actually ready. Uses `TcpListener::bind`
    /// with port 0 (OS-assigned) to avoid port conflicts across parallel
    /// tests and TIME_WAIT leftovers. The test-only variant replaces the
    /// C ABI entrypoint (which hardcodes `0.0.0.0:<port>`) so tests
    /// don't need to speculate about port availability.
    fn start_test_server(srv: *const Server) -> u16 {
        // Ensure libgc is initialized and this test binary's threads may
        // register. `handle_connection` calls `GC_malloc` to build the
        // Request struct; an unregistered thread calling GC_malloc races
        // with the conservative scanner (symptom: "Exclusion ranges
        // overlap" abort under parallel `cargo test`).
        //
        // `tyra_rt_init` is guarded by a `Once` + `OnceLock`, so
        // concurrent tests calling this helper all observe the same
        // init. `GC_register_my_thread` is documented as idempotent
        // per-thread; multiple tests each registering their own listener
        // thread do not interfere.
        crate::tyra_rt_init();
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let srv_ptr = srv as usize;
        thread::spawn(move || {
            crate::gc::register_this_thread();
            let srv = unsafe { &*(srv_ptr as *const Server) };
            for stream in listener.incoming() {
                if let Ok(mut s) = stream {
                    handle_connection(srv, &mut s);
                }
            }
        });
        port
    }

    #[test]
    #[ignore = "needs TCP bind; http.server is experimental in v0.1.0 — run with --ignored locally"]
    fn server_routes_and_responds() {
        let srv = tyra_http_server_new();
        let method = CString::new("GET").unwrap();
        let path = CString::new("/hello").unwrap();
        unsafe { tyra_http_server_route(srv, method.as_ptr(), path.as_ptr(), echo_handler) };

        let port = start_test_server(srv);
        let mut s = TcpStream::connect(("127.0.0.1", port)).unwrap();
        s.write_all(b"GET /hello HTTP/1.1\r\nHost: x\r\n\r\n")
            .unwrap();
        let mut buf = String::new();
        s.read_to_string(&mut buf).unwrap();
        assert!(buf.starts_with("HTTP/1.1 200 OK"), "got: {buf}");
        assert!(buf.contains("echo /hello"), "got: {buf}");
    }

    #[test]
    #[ignore = "needs TCP bind; http.server is experimental in v0.1.0 — run with --ignored locally"]
    fn server_returns_404_for_unknown_route() {
        let srv = tyra_http_server_new();
        let port = start_test_server(srv);
        let mut s = TcpStream::connect(("127.0.0.1", port)).unwrap();
        s.write_all(b"GET /nope HTTP/1.1\r\nHost: x\r\n\r\n")
            .unwrap();
        let mut buf = String::new();
        s.read_to_string(&mut buf).unwrap();
        assert!(buf.starts_with("HTTP/1.1 404"), "got: {buf}");
    }

    extern "C" fn null_handler(_: *const TyraRequest) -> *const TyraResponse {
        std::ptr::null()
    }

    #[test]
    #[ignore = "needs TCP bind; http.server is experimental in v0.1.0 — run with --ignored locally"]
    fn handler_returning_null_gets_500() {
        let srv = tyra_http_server_new();
        let method = CString::new("GET").unwrap();
        let path = CString::new("/bad").unwrap();
        unsafe {
            tyra_http_server_route(srv, method.as_ptr(), path.as_ptr(), null_handler);
        }
        let port = start_test_server(srv);
        let mut s = TcpStream::connect(("127.0.0.1", port)).unwrap();
        s.write_all(b"GET /bad HTTP/1.1\r\nHost: x\r\n\r\n")
            .unwrap();
        let mut buf = String::new();
        s.read_to_string(&mut buf).unwrap();
        assert!(buf.starts_with("HTTP/1.1 500"), "got: {buf}");
    }

    /// Layout cross-check — Rust side. `TyraRequest` / `TyraResponse`
    /// must match Tyra's codegen-emitted struct order for `data Request`
    /// / `data Response`. This test validates only the Rust offsets; a
    /// companion test in `tyra-mir` (`http_server_tyra_struct_field_order`)
    /// lower-parses `stdlib/http/server.ty` and asserts the Tyra
    /// declaration order, so drift on either side trips a failing test.
    #[test]
    fn ffi_layout_matches_tyra_data_order() {
        use std::mem::{offset_of, size_of};
        // Request { method: String, path: String, body: String }
        // All fields are ptr-sized on LP64 (String is a C pointer).
        assert_eq!(size_of::<TyraRequest>(), 3 * size_of::<*const c_char>());
        assert_eq!(offset_of!(TyraRequest, method), 0);
        assert_eq!(offset_of!(TyraRequest, path), size_of::<*const c_char>());
        assert_eq!(
            offset_of!(TyraRequest, body),
            2 * size_of::<*const c_char>()
        );
        // Response { status: Int, body: String }
        // Int is i64; body is ptr. Both 8 bytes on LP64.
        assert_eq!(size_of::<TyraResponse>(), 8 + size_of::<*const c_char>());
        assert_eq!(offset_of!(TyraResponse, status), 0);
        assert_eq!(offset_of!(TyraResponse, body), 8);
    }
}
