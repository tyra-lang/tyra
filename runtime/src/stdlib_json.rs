//! JSON stdlib backing (§22, Tier 2). M10 phase 2; GC-node rewrite (Phase 3
//! permanent-leak elimination).
//!
//! Exposes a hand-rolled JSON parser + node accessors as C ABI intrinsics
//! for `stdlib/json.ty`. Parsing builds an ordinary Rust `JsonValue` tree
//! (unchanged below), then `to_gc` converts it into a tree of GC-managed
//! `JsonNode` structs. The handle Tyra sees — for the root *and* every
//! child returned by `get`/`at` — is simply the address of a `JsonNode`
//! allocated via `GC_malloc`. Handle `0` is reserved for "error / not
//! present".
//!
//! Design: no finalizers. Every handle is a GC pointer in its own right,
//! so a live child handle roots its own subtree under the Boehm
//! collector's conservative stack/register scan — there is no
//! interior-pointer-into-a-leaked-root hazard to guard against, and no
//! extra FFI surface for explicit deallocation. A `JsonNode` is written
//! once during `to_gc` and never mutated afterwards, so it is safe to
//! read concurrently from Tyra worker threads (§14.4 spawn) — those
//! threads are GC-registered (see `runtime/src/lib.rs`'s `gc` module),
//! so their references keep referenced subtrees alive too.
//!
//! Nodes and the pointer arrays backing arrays/objects are allocated via
//! `GC_malloc` (scanned — they hold pointers the collector must trace);
//! string payloads (`Str` values and object keys) are allocated via
//! `gc_string::alloc_gc_cstring` (`GC_malloc_atomic` — no interior
//! pointers). Parse failures never touch GC memory in a way that
//! outlives the call: on failure `tyra_json_parse` returns `0` before
//! any partial `JsonNode` tree is reachable from a returned handle, so
//! any allocations made while converting a value that later failed
//! become ordinary unreferenced garbage — collected, not leaked. If a
//! `GC_malloc` call itself returns null (allocator failure), conversion
//! aborts the same way: the existing parse-error state is set and `0`
//! is returned. No panics cross the C ABI.
//!
//! v0.1 limitations (unrelated to memory management):
//! - Numbers are parsed as `i64` only. JSON floats are currently rejected
//!   by the parser; they surface as `ParseFailed`. Revisit when Tyra grows
//!   a proper `Float` accessor API.
//! - No streaming / incremental parse. Whole document must fit in memory.
//! - Object lookup is linear; O(n) per `get`. Fine for config-sized docs.
//!
//! Error state (`tyra_json_err_*`) is thread-local. The error message is
//! copied into a fresh GC-managed allocation on each read (via
//! `alloc_gc_cstring`) so the caller owns an independent copy and the
//! thread-local can be safely overwritten by a subsequent
//! `tyra_json_parse` without invalidating outstanding pointers.
//!
//! `JsonNode`'s scalar fields (`tag`/`num`/`len`) live in a `GC_malloc`
//! block, so the collector conservatively scans them too — an `i64` value
//! that happens to fall inside the GC's address range can cause bounded
//! false retention, the same inherent conservative-GC trade-off as an
//! ordinary Tyra `Int` field inside a data struct.

use crate::gc_string::{alloc_gc_cstring, try_alloc_gc_cstring};
use std::cell::RefCell;
use std::ffi::{CStr, CString};
use std::os::raw::{c_char, c_int, c_void};
use std::ptr;

// ---------------------------------------------------------------------------
// AST
// ---------------------------------------------------------------------------

pub(crate) enum JsonValue {
    Null,
    Bool(bool),
    Int(i64),
    Str(CString),
    Array(Vec<JsonValue>),
    Object(Vec<(CString, JsonValue)>),
}

// `JsonValue` is Send + Sync automatically because every field is Send + Sync
// (CString, Vec<T: Send+Sync>). That only covers the
// transient parse-time AST, though — it never crosses the C ABI. The
// handles Tyra actually sees are `*const JsonNode` GC pointers (produced
// by `to_gc` below) bit-cast to `i64`, not addresses into this `JsonValue`
// tree and not leaked `Box<JsonValue>` memory (the design this module
// replaced). A `JsonNode` is written once by `to_gc` and never mutated
// afterwards, so aliasing it from multiple threads is safe by
// construction; Tyra's worker threads are GC-registered (see the `gc`
// module in `runtime/src/lib.rs`), so a reference held by one keeps the
// referenced subtree reachable for the collector too.

// ---------------------------------------------------------------------------
// Parser
// ---------------------------------------------------------------------------

struct Parser<'a> {
    bytes: &'a [u8],
    pos: usize,
    line: usize,
    col: usize,
}

#[derive(Debug)]
struct ParseErr {
    msg: String,
    line: usize,
    col: usize,
}

impl<'a> Parser<'a> {
    fn new(bytes: &'a [u8]) -> Self {
        Parser {
            bytes,
            pos: 0,
            line: 1,
            col: 1,
        }
    }

    fn err(&self, msg: impl Into<String>) -> ParseErr {
        ParseErr {
            msg: msg.into(),
            line: self.line,
            col: self.col,
        }
    }

    fn peek(&self) -> Option<u8> {
        self.bytes.get(self.pos).copied()
    }

    fn bump(&mut self) -> Option<u8> {
        let b = self.peek()?;
        self.pos += 1;
        if b == b'\n' {
            self.line += 1;
            self.col = 1;
        } else {
            self.col += 1;
        }
        Some(b)
    }

    fn skip_ws(&mut self) {
        while let Some(b) = self.peek() {
            if b == b' ' || b == b'\t' || b == b'\n' || b == b'\r' {
                self.bump();
            } else {
                break;
            }
        }
    }

    fn expect(&mut self, b: u8) -> Result<(), ParseErr> {
        match self.peek() {
            Some(c) if c == b => {
                self.bump();
                Ok(())
            }
            Some(c) => Err(self.err(format!("expected '{}', got '{}'", b as char, c as char))),
            None => Err(self.err(format!("expected '{}', got EOF", b as char))),
        }
    }

    fn parse_value(&mut self) -> Result<JsonValue, ParseErr> {
        self.skip_ws();
        match self.peek() {
            Some(b'{') => self.parse_object(),
            Some(b'[') => self.parse_array(),
            Some(b'"') => self.parse_string().map(JsonValue::Str),
            Some(b't') | Some(b'f') => self.parse_bool(),
            Some(b'n') => self.parse_null(),
            Some(c) if c == b'-' || c.is_ascii_digit() => self.parse_number(),
            Some(c) => Err(self.err(format!("unexpected character '{}'", c as char))),
            None => Err(self.err("unexpected EOF")),
        }
    }

    fn parse_null(&mut self) -> Result<JsonValue, ParseErr> {
        self.literal(b"null")?;
        Ok(JsonValue::Null)
    }

    fn parse_bool(&mut self) -> Result<JsonValue, ParseErr> {
        match self.peek() {
            Some(b't') => {
                self.literal(b"true")?;
                Ok(JsonValue::Bool(true))
            }
            _ => {
                self.literal(b"false")?;
                Ok(JsonValue::Bool(false))
            }
        }
    }

    fn literal(&mut self, lit: &[u8]) -> Result<(), ParseErr> {
        for &b in lit {
            match self.bump() {
                Some(c) if c == b => {}
                _ => {
                    return Err(self.err(format!(
                        "expected '{}'",
                        std::str::from_utf8(lit).unwrap_or("?")
                    )));
                }
            }
        }
        Ok(())
    }

    fn parse_number(&mut self) -> Result<JsonValue, ParseErr> {
        let start = self.pos;
        if self.peek() == Some(b'-') {
            self.bump();
        }
        let mut has_digit = false;
        while let Some(b) = self.peek() {
            if b.is_ascii_digit() {
                self.bump();
                has_digit = true;
            } else {
                break;
            }
        }
        if !has_digit {
            return Err(self.err("invalid number"));
        }
        // Reject floats for v0.1 — surface as parse error so caller knows.
        if matches!(self.peek(), Some(b'.') | Some(b'e') | Some(b'E')) {
            return Err(self.err("floats not supported in v0.1"));
        }
        let slice = &self.bytes[start..self.pos];
        let s = std::str::from_utf8(slice).map_err(|_| self.err("invalid UTF-8 in number"))?;
        let n: i64 = s.parse().map_err(|_| self.err("integer overflow"))?;
        Ok(JsonValue::Int(n))
    }

    /// Parse four hex digits (RFC 8259 §7 \uXXXX).
    fn parse_hex4(&mut self) -> Result<u32, ParseErr> {
        let mut cp: u32 = 0;
        for _ in 0..4 {
            let d = self
                .bump()
                .ok_or_else(|| self.err("unterminated \\u escape"))?;
            let v = match d {
                b'0'..=b'9' => (d - b'0') as u32,
                b'a'..=b'f' => (d - b'a' + 10) as u32,
                b'A'..=b'F' => (d - b'A' + 10) as u32,
                _ => return Err(self.err("invalid \\u hex digit")),
            };
            cp = (cp << 4) | v;
        }
        Ok(cp)
    }

    fn parse_string(&mut self) -> Result<CString, ParseErr> {
        self.expect(b'"')?;
        let mut buf: Vec<u8> = Vec::new();
        loop {
            match self.bump() {
                Some(b'"') => break,
                Some(b'\\') => {
                    match self.bump() {
                        Some(b'"') => buf.push(b'"'),
                        Some(b'\\') => buf.push(b'\\'),
                        Some(b'/') => buf.push(b'/'),
                        Some(b'n') => buf.push(b'\n'),
                        Some(b't') => buf.push(b'\t'),
                        Some(b'r') => buf.push(b'\r'),
                        Some(b'b') => buf.push(0x08),
                        Some(b'f') => buf.push(0x0c),
                        Some(b'u') => {
                            let cp = self.parse_hex4()?;
                            // RFC 8259 §7: surrogate pair support.
                            // High surrogate U+D800..=U+DBFF must be followed
                            // by \u and a low surrogate U+DC00..=U+DFFF.
                            let final_cp = if (0xD800..=0xDBFF).contains(&cp) {
                                if self.bump() != Some(b'\\') || self.bump() != Some(b'u') {
                                    return Err(self
                                        .err("high surrogate not followed by \\u low surrogate"));
                                }
                                let low = self.parse_hex4()?;
                                if !(0xDC00..=0xDFFF).contains(&low) {
                                    return Err(self.err("invalid low surrogate"));
                                }
                                0x10000 + ((cp - 0xD800) << 10) + (low - 0xDC00)
                            } else if (0xDC00..=0xDFFF).contains(&cp) {
                                return Err(self.err("unpaired low surrogate"));
                            } else {
                                cp
                            };
                            if let Some(ch) = char::from_u32(final_cp) {
                                let mut tmp = [0u8; 4];
                                let s = ch.encode_utf8(&mut tmp);
                                buf.extend_from_slice(s.as_bytes());
                            } else {
                                return Err(self.err("invalid unicode escape"));
                            }
                        }
                        Some(c) => return Err(self.err(format!("invalid escape \\{}", c as char))),
                        None => return Err(self.err("unterminated string escape")),
                    }
                }
                Some(0) => return Err(self.err("NUL byte in string")),
                Some(b) => buf.push(b),
                None => return Err(self.err("unterminated string")),
            }
        }
        CString::new(buf).map_err(|_| self.err("interior NUL in string"))
    }

    fn parse_array(&mut self) -> Result<JsonValue, ParseErr> {
        self.expect(b'[')?;
        let mut items: Vec<JsonValue> = Vec::new();
        self.skip_ws();
        if self.peek() == Some(b']') {
            self.bump();
            return Ok(JsonValue::Array(items));
        }
        loop {
            let v = self.parse_value()?;
            items.push(v);
            self.skip_ws();
            match self.peek() {
                Some(b',') => {
                    self.bump();
                }
                Some(b']') => {
                    self.bump();
                    break;
                }
                Some(c) => {
                    return Err(
                        self.err(format!("expected ',' or ']' in array, got '{}'", c as char))
                    );
                }
                None => return Err(self.err("unterminated array")),
            }
        }
        Ok(JsonValue::Array(items))
    }

    fn parse_object(&mut self) -> Result<JsonValue, ParseErr> {
        self.expect(b'{')?;
        let mut items: Vec<(CString, JsonValue)> = Vec::new();
        self.skip_ws();
        if self.peek() == Some(b'}') {
            self.bump();
            return Ok(JsonValue::Object(items));
        }
        loop {
            self.skip_ws();
            let key = self.parse_string()?;
            self.skip_ws();
            self.expect(b':')?;
            let v = self.parse_value()?;
            items.push((key, v));
            self.skip_ws();
            match self.peek() {
                Some(b',') => {
                    self.bump();
                }
                Some(b'}') => {
                    self.bump();
                    break;
                }
                Some(c) => {
                    return Err(self.err(format!(
                        "expected ',' or '}}' in object, got '{}'",
                        c as char
                    )));
                }
                None => return Err(self.err("unterminated object")),
            }
        }
        Ok(JsonValue::Object(items))
    }

    fn parse_document(&mut self) -> Result<JsonValue, ParseErr> {
        let v = self.parse_value()?;
        self.skip_ws();
        if self.pos < self.bytes.len() {
            return Err(self.err("trailing content after JSON value"));
        }
        Ok(v)
    }
}

// ---------------------------------------------------------------------------
// GC node tree
// ---------------------------------------------------------------------------

/// GC-managed JSON node. `tag` selects which of `num` / `str_` /
/// `items`+`len` / `items`+`keys`+`len` is meaningful:
///   0 Null   — no payload
///   1 Bool   — `num` is 0/1
///   2 Int    — `num` is the value
///   3 Str    — `str_` is a GC cstring (`alloc_gc_cstring`)
///   4 Array  — `items[0..len]` are child `*const JsonNode`
///   5 Object — `keys[0..len]` / `items[0..len]` are parallel GC cstring /
///              child `*const JsonNode` arrays
///
/// Allocated via `GC_malloc` (scanned) so the collector traces `items`
/// and `keys` into their targets. Written once by `to_gc`, then treated
/// as immutable — safe to alias and read from multiple threads.
#[repr(C)]
struct JsonNode {
    tag: i64,
    num: i64,
    str_: *const c_char,
    len: i64,
    items: *const *const JsonNode,
    keys: *const *const c_char,
}

const TAG_NULL: i64 = 0;
const TAG_BOOL: i64 = 1;
const TAG_INT: i64 = 2;
const TAG_STR: i64 = 3;
const TAG_ARRAY: i64 = 4;
const TAG_OBJECT: i64 = 5;

/// Allocate a single zeroed `JsonNode` in GC memory. Returns `None` on
/// allocator failure (propagated by `to_gc` as a whole-conversion
/// failure — see module doc).
fn alloc_node() -> Option<*mut JsonNode> {
    let p = crate::gc::malloc(std::mem::size_of::<JsonNode>()) as *mut JsonNode;
    if p.is_null() { None } else { Some(p) }
}

/// Allocate a GC-managed, scanned array of `n` pointers. Used for both
/// `JsonNode::items` (`*const JsonNode` elements) and `JsonNode::keys`
/// (`*const c_char` elements) — both are pointer-sized, so one helper
/// covers both, with callers casting the result to the field's type.
/// Returns `None` on allocator failure.
fn alloc_ptr_array(n: usize) -> Option<*mut *const c_void> {
    let size = n * std::mem::size_of::<*const c_void>();
    let p = crate::gc::malloc(size) as *mut *const c_void;
    if p.is_null() { None } else { Some(p) }
}

/// Convert a parsed `JsonValue` tree into a `JsonNode` tree living in
/// GC memory, returning the root's address. Returns `None` if any
/// `GC_malloc` call along the way fails — callers must not treat a
/// partial result as valid; see module doc for the failure-path
/// contract (no leaks, no panics).
fn to_gc(v: &JsonValue) -> Option<*const JsonNode> {
    let node = alloc_node()?;
    // SAFETY: `node` was just allocated by us and is not yet visible to
    // any other thread or to Tyra (its address hasn't been returned as
    // a handle yet), so exclusive access here is sound. `GC_malloc`
    // zero-initializes, so fields left untouched below are already 0 /
    // null; we still set every field explicitly for clarity.
    unsafe {
        match v {
            JsonValue::Null => {
                (*node).tag = TAG_NULL;
                (*node).num = 0;
                (*node).str_ = ptr::null();
                (*node).len = 0;
                (*node).items = ptr::null();
                (*node).keys = ptr::null();
            }
            JsonValue::Bool(b) => {
                (*node).tag = TAG_BOOL;
                (*node).num = *b as i64;
                (*node).str_ = ptr::null();
                (*node).len = 0;
                (*node).items = ptr::null();
                (*node).keys = ptr::null();
            }
            JsonValue::Int(n) => {
                (*node).tag = TAG_INT;
                (*node).num = *n;
                (*node).str_ = ptr::null();
                (*node).len = 0;
                (*node).items = ptr::null();
                (*node).keys = ptr::null();
            }
            JsonValue::Str(s) => {
                (*node).tag = TAG_STR;
                (*node).num = 0;
                // `?`: an OOM here (GC_malloc_atomic failure) aborts this
                // whole conversion the same way a `to_gc` failure deeper
                // in the tree does — see module doc's failure-path
                // contract (no leaks, no panics, caller sees `to_gc`
                // return `None`).
                (*node).str_ = try_alloc_gc_cstring(s.to_str().unwrap_or(""))?;
                (*node).len = 0;
                (*node).items = ptr::null();
                (*node).keys = ptr::null();
            }
            JsonValue::Array(items) => {
                (*node).tag = TAG_ARRAY;
                (*node).num = 0;
                (*node).str_ = ptr::null();
                (*node).len = items.len() as i64;
                (*node).keys = ptr::null();
                if items.is_empty() {
                    (*node).items = ptr::null();
                } else {
                    let arr = alloc_ptr_array(items.len())?;
                    for (i, item) in items.iter().enumerate() {
                        let child = to_gc(item)?;
                        *arr.add(i) = child as *const c_void;
                    }
                    (*node).items = arr as *const *const JsonNode;
                }
            }
            JsonValue::Object(entries) => {
                (*node).tag = TAG_OBJECT;
                (*node).num = 0;
                (*node).str_ = ptr::null();
                (*node).len = entries.len() as i64;
                if entries.is_empty() {
                    (*node).items = ptr::null();
                    (*node).keys = ptr::null();
                } else {
                    let items_arr = alloc_ptr_array(entries.len())?;
                    let keys_arr = alloc_ptr_array(entries.len())?;
                    for (i, (k, val)) in entries.iter().enumerate() {
                        // Same OOM-propagation reasoning as the `Str` arm
                        // above: an allocator failure on an object key
                        // aborts the whole conversion via `?` rather than
                        // silently falling back to an empty-string key.
                        *keys_arr.add(i) =
                            try_alloc_gc_cstring(k.to_str().unwrap_or(""))? as *const c_void;
                        let child = to_gc(val)?;
                        *items_arr.add(i) = child as *const c_void;
                    }
                    (*node).items = items_arr as *const *const JsonNode;
                    (*node).keys = keys_arr as *const *const c_char;
                }
            }
        }
    }
    Some(node as *const JsonNode)
}

// ---------------------------------------------------------------------------
// Error state (thread-local)
// ---------------------------------------------------------------------------

thread_local! {
    static LAST_ERR: RefCell<Option<(String, i64, i64)>> = const { RefCell::new(None) };
}

fn set_err(e: ParseErr) {
    LAST_ERR.with(|s| *s.borrow_mut() = Some((e.msg, e.line as i64, e.col as i64)));
}

// ---------------------------------------------------------------------------
// C ABI
// ---------------------------------------------------------------------------

/// Parse a JSON document. Returns an opaque handle (non-zero) on success,
/// or 0 on failure with details available via `tyra_json_err_*`.
///
/// On failure, no memory is leaked in the permanent sense: the Rust
/// `JsonValue` tree built by the parser drops normally, and if a
/// partial `JsonNode` GC tree was built before a `GC_malloc` failure it
/// is simply unreferenced garbage — collected on the next GC cycle, not
/// leaked. Every *successful* parse is GC-managed too: the whole tree
/// (root and every node reachable from it) is reclaimed once no handle
/// referencing it remains reachable.
///
/// # Safety
/// `text` must be a null-terminated UTF-8 string.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tyra_json_parse(text: *const c_char) -> i64 {
    if text.is_null() {
        set_err(ParseErr {
            msg: "null input".into(),
            line: 0,
            col: 0,
        });
        return 0;
    }
    let bytes = unsafe { CStr::from_ptr(text) }.to_bytes();
    let mut p = Parser::new(bytes);
    match p.parse_document() {
        Ok(v) => match to_gc(&v) {
            Some(node) => node as i64,
            None => {
                set_err(ParseErr {
                    msg: "out of memory (GC allocation failed)".into(),
                    line: 0,
                    col: 0,
                });
                0
            }
        },
        Err(e) => {
            set_err(e);
            0
        }
    }
}

/// Return the last parse error message as a fresh GC-managed C string
/// (via `alloc_gc_cstring`). Returns non-null. There is no caller-owned
/// leak to accept: unlike the `CString::into_raw` pattern used elsewhere
/// in this crate (e.g. `stdlib_http`'s `HTTP_ERRMSG_PTR`), the returned
/// pointer is reclaimed by the Boehm collector like any other GC
/// allocation once it becomes unreachable — no explicit free, and no
/// unbounded growth from repeated calls.
///
/// Copying out on every read decouples the caller from the thread-local
/// lifetime — subsequent `tyra_json_parse` calls cannot invalidate the
/// returned pointer.
#[unsafe(no_mangle)]
pub extern "C" fn tyra_json_err_msg() -> *const c_char {
    let s = LAST_ERR.with(|slot| {
        slot.borrow()
            .as_ref()
            .map(|(m, _, _)| m.clone())
            .unwrap_or_default()
    });
    alloc_gc_cstring(&s)
}

#[unsafe(no_mangle)]
pub extern "C" fn tyra_json_err_line() -> i64 {
    LAST_ERR.with(|s| s.borrow().as_ref().map(|(_, l, _)| *l).unwrap_or(0))
}

#[unsafe(no_mangle)]
pub extern "C" fn tyra_json_err_col() -> i64 {
    LAST_ERR.with(|s| s.borrow().as_ref().map(|(_, _, c)| *c).unwrap_or(0))
}

/// Resolve a handle to a `&'static JsonNode`. The `'static` lifetime is
/// honest in the GC sense: as long as this handle (or a copy of it) is
/// reachable — e.g. held in a local — the Boehm collector will not
/// reclaim the node. Callers must only pass handles produced by
/// `tyra_json_parse` or one of the child accessors; `0` returns `None`.
unsafe fn node_ref(h: i64) -> Option<&'static JsonNode> {
    if h == 0 {
        return None;
    }
    Some(unsafe { &*(h as *const JsonNode) })
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn tyra_json_kind(h: i64) -> *const c_char {
    let name: &[u8] = match unsafe { node_ref(h) }.map(|n| n.tag) {
        Some(TAG_NULL) | None => b"null\0",
        Some(TAG_BOOL) => b"bool\0",
        Some(TAG_INT) => b"int\0",
        Some(TAG_STR) => b"string\0",
        Some(TAG_ARRAY) => b"array\0",
        Some(TAG_OBJECT) => b"object\0",
        Some(_) => b"null\0",
    };
    name.as_ptr() as *const c_char
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn tyra_json_is_string(h: i64) -> c_int {
    (unsafe { node_ref(h) }.map(|n| n.tag) == Some(TAG_STR)) as c_int
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn tyra_json_is_int(h: i64) -> c_int {
    (unsafe { node_ref(h) }.map(|n| n.tag) == Some(TAG_INT)) as c_int
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn tyra_json_is_bool(h: i64) -> c_int {
    (unsafe { node_ref(h) }.map(|n| n.tag) == Some(TAG_BOOL)) as c_int
}

/// Must be called only when `tyra_json_is_string(h)` returned 1. Returns
/// a pointer to the node's GC-managed string payload; caller must not
/// free it.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tyra_json_str(h: i64) -> *const c_char {
    match unsafe { node_ref(h) } {
        Some(n) if n.tag == TAG_STR => n.str_,
        _ => c"".as_ptr(),
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn tyra_json_int(h: i64) -> i64 {
    match unsafe { node_ref(h) } {
        Some(n) if n.tag == TAG_INT => n.num,
        _ => 0,
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn tyra_json_bool(h: i64) -> c_int {
    match unsafe { node_ref(h) } {
        Some(n) if n.tag == TAG_BOOL => n.num as c_int,
        _ => 0,
    }
}

/// Object lookup. Returns child handle, or 0 if not an object / key missing.
///
/// # Safety
/// `key` must be a null-terminated UTF-8 string.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tyra_json_get(h: i64, key: *const c_char) -> i64 {
    if key.is_null() {
        return 0;
    }
    let key_bytes = unsafe { CStr::from_ptr(key) }.to_bytes();
    match unsafe { node_ref(h) } {
        Some(n) if n.tag == TAG_OBJECT => {
            let len = n.len as usize;
            for i in 0..len {
                let k_ptr = unsafe { *n.keys.add(i) };
                let k_bytes = unsafe { CStr::from_ptr(k_ptr) }.to_bytes();
                if k_bytes == key_bytes {
                    let child = unsafe { *n.items.add(i) };
                    return child as i64;
                }
            }
            0
        }
        _ => 0,
    }
}

/// Array index. Returns child handle, or 0 if not an array / out of bounds.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tyra_json_at(h: i64, index: i64) -> i64 {
    match unsafe { node_ref(h) } {
        Some(n) if n.tag == TAG_ARRAY => {
            if index < 0 {
                return 0;
            }
            let i = index as usize;
            if i >= n.len as usize {
                return 0;
            }
            let child = unsafe { *n.items.add(i) };
            child as i64
        }
        _ => 0,
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn cstr(s: &str) -> CString {
        CString::new(s).unwrap()
    }

    // Kept as a call-site marker in the tests below (documents "this
    // pointer's lifetime ends here" at each call), but does nothing: every
    // `tyra_json_err_msg()` pointer is a GC allocation (`alloc_gc_cstring`),
    // reclaimed by the collector once unreachable, not a caller-owned
    // buffer requiring an explicit free.
    fn free_errmsg(_p: *const c_char) {}

    #[test]
    fn parse_object_and_get() {
        let input = cstr(r#"{"name":"alice","age":30}"#);
        let h = unsafe { tyra_json_parse(input.as_ptr()) };
        assert!(h != 0);
        let k = cstr("name");
        let name_h = unsafe { tyra_json_get(h, k.as_ptr()) };
        assert!(name_h != 0);
        assert_eq!(unsafe { tyra_json_is_string(name_h) }, 1);
        let s = unsafe { CStr::from_ptr(tyra_json_str(name_h)) }
            .to_str()
            .unwrap();
        assert_eq!(s, "alice");

        let k2 = cstr("age");
        let age_h = unsafe { tyra_json_get(h, k2.as_ptr()) };
        assert_eq!(unsafe { tyra_json_is_int(age_h) }, 1);
        assert_eq!(unsafe { tyra_json_int(age_h) }, 30);
    }

    #[test]
    fn parse_array_and_at() {
        let input = cstr(r#"[true, false, null, "x"]"#);
        let h = unsafe { tyra_json_parse(input.as_ptr()) };
        assert!(h != 0);
        let a0 = unsafe { tyra_json_at(h, 0) };
        assert_eq!(unsafe { tyra_json_is_bool(a0) }, 1);
        assert_eq!(unsafe { tyra_json_bool(a0) }, 1);
        let a3 = unsafe { tyra_json_at(h, 3) };
        assert_eq!(unsafe { tyra_json_is_string(a3) }, 1);
        assert_eq!(unsafe { tyra_json_at(h, 99) }, 0);
    }

    #[test]
    fn parse_error_reports_position() {
        let input = cstr(r#"{"name": oops}"#);
        let h = unsafe { tyra_json_parse(input.as_ptr()) };
        assert_eq!(h, 0);
        let line = tyra_json_err_line();
        let col = tyra_json_err_col();
        assert_eq!(line, 1);
        assert!(col > 0);
        let msg = tyra_json_err_msg();
        let s = unsafe { CStr::from_ptr(msg) }.to_str().unwrap().to_string();
        assert!(!s.is_empty());
        free_errmsg(msg);
    }

    #[test]
    fn parse_error_msg_survives_next_parse() {
        // Regression guard: err_msg must return a caller-owned copy, not a
        // pointer into the thread-local. A subsequent parse must not
        // invalidate the first error message.
        let bad = cstr(r#"{"oops"#);
        let _ = unsafe { tyra_json_parse(bad.as_ptr()) };
        let msg1 = tyra_json_err_msg();
        let good = cstr(r#"{"ok":1}"#);
        let _ = unsafe { tyra_json_parse(good.as_ptr()) };
        let s1 = unsafe { CStr::from_ptr(msg1) }.to_str().unwrap();
        assert!(!s1.is_empty());
        free_errmsg(msg1);
    }

    #[test]
    fn missing_key_returns_zero() {
        let input = cstr(r#"{"a":1}"#);
        let h = unsafe { tyra_json_parse(input.as_ptr()) };
        let k = cstr("missing");
        assert_eq!(unsafe { tyra_json_get(h, k.as_ptr()) }, 0);
    }

    #[test]
    fn string_escapes() {
        let input = cstr(r#""line1\nline2\t\"quoted\"""#);
        let h = unsafe { tyra_json_parse(input.as_ptr()) };
        assert!(h != 0);
        let s = unsafe { CStr::from_ptr(tyra_json_str(h)) }
            .to_str()
            .unwrap();
        assert_eq!(s, "line1\nline2\t\"quoted\"");
    }

    #[test]
    fn surrogate_pair_decodes_to_astral() {
        // "\uD83D\uDE00" is 😀 (U+1F600).
        let input = cstr(r#""\uD83D\uDE00""#);
        let h = unsafe { tyra_json_parse(input.as_ptr()) };
        assert!(h != 0, "surrogate pair must decode successfully");
        let s = unsafe { CStr::from_ptr(tyra_json_str(h)) }
            .to_str()
            .unwrap();
        assert_eq!(s, "😀");
    }

    #[test]
    fn lone_high_surrogate_rejected() {
        let input = cstr(r#""\uD83Dx""#);
        let h = unsafe { tyra_json_parse(input.as_ptr()) };
        assert_eq!(h, 0);
    }

    #[test]
    fn lone_low_surrogate_rejected() {
        let input = cstr(r#""\uDE00""#);
        let h = unsafe { tyra_json_parse(input.as_ptr()) };
        assert_eq!(h, 0);
    }

    #[test]
    fn nested_object() {
        let input = cstr(r#"{"user": {"name": "bob"}}"#);
        let h = unsafe { tyra_json_parse(input.as_ptr()) };
        let u = cstr("user");
        let user_h = unsafe { tyra_json_get(h, u.as_ptr()) };
        let n = cstr("name");
        let name_h = unsafe { tyra_json_get(user_h, n.as_ptr()) };
        let s = unsafe { CStr::from_ptr(tyra_json_str(name_h)) }
            .to_str()
            .unwrap();
        assert_eq!(s, "bob");
    }

    #[test]
    fn kind_strings() {
        let input = cstr(r#"[null, true, 1, "x", [], {}]"#);
        let h = unsafe { tyra_json_parse(input.as_ptr()) };
        let check = |i: i64, want: &str| {
            let c = unsafe { tyra_json_at(h, i) };
            let k = unsafe { CStr::from_ptr(tyra_json_kind(c)) }
                .to_str()
                .unwrap();
            assert_eq!(k, want);
        };
        check(0, "null");
        check(1, "bool");
        check(2, "int");
        check(3, "string");
        check(4, "array");
        check(5, "object");
    }

    #[test]
    fn top_level_null() {
        let input = cstr("null");
        let h = unsafe { tyra_json_parse(input.as_ptr()) };
        assert!(h != 0, "null root must have a non-zero handle");
        let k = unsafe { CStr::from_ptr(tyra_json_kind(h)) }
            .to_str()
            .unwrap();
        assert_eq!(k, "null");
    }

    #[test]
    fn empty_input_rejected() {
        let input = cstr("");
        let h = unsafe { tyra_json_parse(input.as_ptr()) };
        assert_eq!(h, 0);
        let msg = tyra_json_err_msg();
        let s = unsafe { CStr::from_ptr(msg) }.to_str().unwrap().to_string();
        assert!(s.contains("EOF") || s.contains("unexpected"));
        free_errmsg(msg);
    }

    // `heap_growth_bounded_after_many_parses` and
    // `child_survives_after_root_handle_goes_out_of_scope` moved to
    // `runtime/tests/json_heap.rs` (integration test, own process) — see
    // that file's module doc for why: `GC_gcollect()`'s stop-the-world on
    // Darwin aborts once other unregistered libtest threads have run
    // earlier in the same process.
}
