//! JSON stdlib backing (§22, Tier 2). M10 phase 2.
//!
//! Exposes a hand-rolled JSON parser + node accessors as C ABI intrinsics
//! for `stdlib/json.tyra`. The parsed document root is wrapped in `Box` and
//! leaked via `Box::leak`; its raw pointer is handed to Tyra as an `i64`
//! handle. Child nodes live inside the root's allocation as owned `Box`es,
//! so a single root leak keeps the entire tree alive. Handle `0` is
//! reserved for "error / not present".
//!
//! v0.1 limitations:
//! - The root allocation is leaked (same trade-off as `stdlib_fs::tyra_fs_read`
//!   via `CString::into_raw`). Parse is typically called once per file, so
//!   this is acceptable pending GC_malloc integration. **Failed parses do
//!   NOT leak** — partial trees are dropped normally before `tyra_json_parse`
//!   returns 0.
//! - Numbers are parsed as `i64` only. JSON floats are currently rejected
//!   by the parser; they surface as `ParseFailed`. Revisit when Tyra grows
//!   a proper `Float` accessor API.
//! - No streaming / incremental parse. Whole document must fit in memory.
//! - Object lookup is linear; O(n) per `get`. Fine for config-sized docs.
//!
//! Thread-safety: `JsonValue` contains only owned, immutable data
//! (`CString`, `Box<JsonValue>`) — no interior mutability, no raw pointers.
//! Once a root is leaked, the entire tree is effectively `&'static` and
//! safe to read concurrently from Tyra workers (§14.4 spawn). Accessors
//! hand out `'static` references derived from the leaked root.
//!
//! Error state (`tyra_json_err_*`) is thread-local. The error message is
//! copied to a fresh `CString::into_raw` allocation on each read so the
//! caller owns it and the thread-local can be safely overwritten by a
//! subsequent `tyra_json_parse` without invalidating outstanding pointers.

use crate::gc_string::alloc_gc_cstring;
use std::cell::RefCell;
use std::ffi::{CStr, CString};
use std::os::raw::{c_char, c_int};

// ---------------------------------------------------------------------------
// AST
// ---------------------------------------------------------------------------

pub(crate) enum JsonValue {
    Null,
    Bool(bool),
    Int(i64),
    Str(CString),
    Array(Vec<Box<JsonValue>>),
    Object(Vec<(CString, Box<JsonValue>)>),
}

// `JsonValue` is Send + Sync automatically because every field is Send + Sync
// (CString, Box<T: Send+Sync>, Vec<T: Send+Sync>). No raw pointers are stored;
// handles handed to Tyra are i64 bit-casts of leaked `Box<JsonValue>`
// addresses, not raw pointers held by the AST itself.

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
        Parser { bytes, pos: 0, line: 1, col: 1 }
    }

    fn err(&self, msg: impl Into<String>) -> ParseErr {
        ParseErr { msg: msg.into(), line: self.line, col: self.col }
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
            Some(c) if c == b => { self.bump(); Ok(()) }
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
            Some(b't') => { self.literal(b"true")?; Ok(JsonValue::Bool(true)) }
            _ => { self.literal(b"false")?; Ok(JsonValue::Bool(false)) }
        }
    }

    fn literal(&mut self, lit: &[u8]) -> Result<(), ParseErr> {
        for &b in lit {
            match self.bump() {
                Some(c) if c == b => {}
                _ => return Err(self.err(format!(
                    "expected '{}'", std::str::from_utf8(lit).unwrap_or("?")
                ))),
            }
        }
        Ok(())
    }

    fn parse_number(&mut self) -> Result<JsonValue, ParseErr> {
        let start = self.pos;
        if self.peek() == Some(b'-') { self.bump(); }
        let mut has_digit = false;
        while let Some(b) = self.peek() {
            if b.is_ascii_digit() { self.bump(); has_digit = true; } else { break; }
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
            let d = self.bump().ok_or_else(|| self.err("unterminated \\u escape"))?;
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
                                    return Err(self.err("high surrogate not followed by \\u low surrogate"));
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
                Some(b) if b == 0 => return Err(self.err("NUL byte in string")),
                Some(b) => buf.push(b),
                None => return Err(self.err("unterminated string")),
            }
        }
        CString::new(buf).map_err(|_| self.err("interior NUL in string"))
    }

    fn parse_array(&mut self) -> Result<JsonValue, ParseErr> {
        self.expect(b'[')?;
        let mut items: Vec<Box<JsonValue>> = Vec::new();
        self.skip_ws();
        if self.peek() == Some(b']') { self.bump(); return Ok(JsonValue::Array(items)); }
        loop {
            let v = self.parse_value()?;
            items.push(Box::new(v));
            self.skip_ws();
            match self.peek() {
                Some(b',') => { self.bump(); }
                Some(b']') => { self.bump(); break; }
                Some(c) => return Err(self.err(format!("expected ',' or ']' in array, got '{}'", c as char))),
                None => return Err(self.err("unterminated array")),
            }
        }
        Ok(JsonValue::Array(items))
    }

    fn parse_object(&mut self) -> Result<JsonValue, ParseErr> {
        self.expect(b'{')?;
        let mut items: Vec<(CString, Box<JsonValue>)> = Vec::new();
        self.skip_ws();
        if self.peek() == Some(b'}') { self.bump(); return Ok(JsonValue::Object(items)); }
        loop {
            self.skip_ws();
            let key = self.parse_string()?;
            self.skip_ws();
            self.expect(b':')?;
            let v = self.parse_value()?;
            items.push((key, Box::new(v)));
            self.skip_ws();
            match self.peek() {
                Some(b',') => { self.bump(); }
                Some(b'}') => { self.bump(); break; }
                Some(c) => return Err(self.err(format!("expected ',' or '}}' in object, got '{}'", c as char))),
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
/// On failure, no memory is leaked: partial trees are dropped normally
/// before returning. Only successful parses leak (the root Box).
///
/// # Safety
/// `text` must be a null-terminated UTF-8 string.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tyra_json_parse(text: *const c_char) -> i64 {
    if text.is_null() {
        set_err(ParseErr { msg: "null input".into(), line: 0, col: 0 });
        return 0;
    }
    let bytes = unsafe { CStr::from_ptr(text) }.to_bytes();
    let mut p = Parser::new(bytes);
    match p.parse_document() {
        Ok(v) => {
            // Leak exactly once, at the root. All children live inside this
            // allocation (via owned Boxes) and become effectively `'static`.
            Box::leak(Box::new(v)) as *const JsonValue as i64
        }
        Err(e) => { set_err(e); 0 }
    }
}

/// Return the last parse error message as a heap-allocated C string owned
/// by the caller. Returns non-null; leak on drop is accepted v0.1.
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

/// Resolve a handle to a `&'static JsonValue`. The `'static` lifetime is
/// honest: nodes live inside the leaked root `Box` and are never freed.
/// Callers must only pass handles produced by `tyra_json_parse` or one of
/// the child accessors; `0` returns `None`.
unsafe fn node_ref(h: i64) -> Option<&'static JsonValue> {
    if h == 0 { return None; }
    Some(unsafe { &*(h as *const JsonValue) })
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn tyra_json_kind(h: i64) -> *const c_char {
    let name: &[u8] = match unsafe { node_ref(h) } {
        Some(JsonValue::Null) => b"null\0",
        Some(JsonValue::Bool(_)) => b"bool\0",
        Some(JsonValue::Int(_)) => b"int\0",
        Some(JsonValue::Str(_)) => b"string\0",
        Some(JsonValue::Array(_)) => b"array\0",
        Some(JsonValue::Object(_)) => b"object\0",
        None => b"null\0",
    };
    name.as_ptr() as *const c_char
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn tyra_json_is_string(h: i64) -> c_int {
    matches!(unsafe { node_ref(h) }, Some(JsonValue::Str(_))) as c_int
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn tyra_json_is_int(h: i64) -> c_int {
    matches!(unsafe { node_ref(h) }, Some(JsonValue::Int(_))) as c_int
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn tyra_json_is_bool(h: i64) -> c_int {
    matches!(unsafe { node_ref(h) }, Some(JsonValue::Bool(_))) as c_int
}

/// Must be called only when `tyra_json_is_string(h)` returned 1. Returns
/// a pointer to the stored CString; caller must not free it.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tyra_json_str(h: i64) -> *const c_char {
    match unsafe { node_ref(h) } {
        Some(JsonValue::Str(s)) => s.as_ptr(),
        _ => b"\0".as_ptr() as *const c_char,
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn tyra_json_int(h: i64) -> i64 {
    match unsafe { node_ref(h) } {
        Some(JsonValue::Int(n)) => *n,
        _ => 0,
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn tyra_json_bool(h: i64) -> c_int {
    match unsafe { node_ref(h) } {
        Some(JsonValue::Bool(b)) => *b as c_int,
        _ => 0,
    }
}

/// Object lookup. Returns child handle, or 0 if not an object / key missing.
///
/// # Safety
/// `key` must be a null-terminated UTF-8 string.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tyra_json_get(h: i64, key: *const c_char) -> i64 {
    if key.is_null() { return 0; }
    let key_bytes = unsafe { CStr::from_ptr(key) }.to_bytes();
    match unsafe { node_ref(h) } {
        Some(JsonValue::Object(entries)) => {
            for (k, v) in entries {
                if k.as_bytes() == key_bytes {
                    return (&**v) as *const JsonValue as i64;
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
        Some(JsonValue::Array(items)) => {
            if index < 0 { return 0; }
            let i = index as usize;
            items
                .get(i)
                .map(|b| (&**b) as *const JsonValue as i64)
                .unwrap_or(0)
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

    fn cstr(s: &str) -> CString { CString::new(s).unwrap() }

    // errmsg pointers are GC-managed; no manual free needed.
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
        let s = unsafe { CStr::from_ptr(tyra_json_str(name_h)) }.to_str().unwrap();
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
        unsafe { free_errmsg(msg) };
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
        unsafe { free_errmsg(msg1) };
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
        let s = unsafe { CStr::from_ptr(tyra_json_str(h)) }.to_str().unwrap();
        assert_eq!(s, "line1\nline2\t\"quoted\"");
    }

    #[test]
    fn surrogate_pair_decodes_to_astral() {
        // "\uD83D\uDE00" is 😀 (U+1F600).
        let input = cstr(r#""\uD83D\uDE00""#);
        let h = unsafe { tyra_json_parse(input.as_ptr()) };
        assert!(h != 0, "surrogate pair must decode successfully");
        let s = unsafe { CStr::from_ptr(tyra_json_str(h)) }.to_str().unwrap();
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
        let s = unsafe { CStr::from_ptr(tyra_json_str(name_h)) }.to_str().unwrap();
        assert_eq!(s, "bob");
    }

    #[test]
    fn kind_strings() {
        let input = cstr(r#"[null, true, 1, "x", [], {}]"#);
        let h = unsafe { tyra_json_parse(input.as_ptr()) };
        let check = |i: i64, want: &str| {
            let c = unsafe { tyra_json_at(h, i) };
            let k = unsafe { CStr::from_ptr(tyra_json_kind(c)) }.to_str().unwrap();
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
        let k = unsafe { CStr::from_ptr(tyra_json_kind(h)) }.to_str().unwrap();
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
        unsafe { free_errmsg(msg) };
    }
}
