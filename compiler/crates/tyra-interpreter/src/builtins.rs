// Builtin function implementations for the MIR interpreter.
//
// Covers: print/string/list/float/json-like and collection operations.
// NOT covered: io (stdin), fs, http, async, time — these are rejected at the
// check phase (E0200) or unsupported in the Playground.

use std::cell::RefCell;
use std::rc::Rc;

use crate::printf_fmt::format_float;
use crate::value::{Value, key_cmp, key_eq};

// ── Global errno pattern ─────────────────────────────────────────────────────
// The stdlib uses a C errno-like pattern: __string_char_at(s, idx) stores a
// status code, then __string_char_errno() (no args) reads it.  Likewise for
// __string_parse_errno() and __float_parse_errno().  We implement this via a
// thread-local so the builtins remain stateless from the interpreter's view.
thread_local! {
    static LAST_ERRNO: RefCell<i64> = RefCell::new(0);
}
fn set_errno(code: i64) { LAST_ERRNO.with(|e| *e.borrow_mut() = code); }
fn get_errno() -> i64   { LAST_ERRNO.with(|e| *e.borrow()) }

/// Return type for builtin calls: either a Value or a special signal.
pub enum BuiltinResult {
    Value(Value),
    /// sys.exit(code) — propagate upward immediately.
    Exit(i32),
    /// panic(msg) — propagate upward as exit 101.
    Panic(String),
    /// Builtin not found — caller should look for a user function.
    NotABuiltin,
}

/// Dispatch a builtin call by name.
pub fn call_builtin(
    fname: &str,
    args: Vec<Value>,
    stdout: &mut String,
    stderr: &mut String,
) -> BuiltinResult {
    match fname {
        // ── Print family ────────────────────────────────────────────────────
        "print" => {
            if let Some(arg) = args.into_iter().next() {
                stdout.push_str(&value_to_display(&arg));
            }
            BuiltinResult::Value(Value::Unit)
        }
        "println" => {
            if let Some(arg) = args.into_iter().next() {
                stdout.push_str(&value_to_display(&arg));
            }
            stdout.push('\n');
            BuiltinResult::Value(Value::Unit)
        }
        "eprint" => {
            if let Some(arg) = args.into_iter().next() {
                stderr.push_str(&value_to_display(&arg));
            }
            BuiltinResult::Value(Value::Unit)
        }
        "eprintln" => {
            if let Some(arg) = args.into_iter().next() {
                stderr.push_str(&value_to_display(&arg));
            }
            stderr.push('\n');
            BuiltinResult::Value(Value::Unit)
        }

        // ── Panic / sys.exit ────────────────────────────────────────────────
        "panic" => {
            let msg = args.into_iter().next()
                .map(|v| value_to_display(&v))
                .unwrap_or_default();
            BuiltinResult::Panic(msg)
        }
        "sys__exit" => {
            let code = args.into_iter().next()
                .map(|v| v.as_int() as i32)
                .unwrap_or(0);
            BuiltinResult::Exit(code)
        }

        // ── Float ────────────────────────────────────────────────────────────
        "__float_eq" => {
            let (a, b) = two_floats(args);
            BuiltinResult::Value(Value::Bool(a == b))
        }
        "__float_approx_eq" => {
            let (a, b) = two_floats(args);
            BuiltinResult::Value(Value::Bool((a - b).abs() < 1e-9))
        }
        "__float_abs" => BuiltinResult::Value(Value::Float(args[0].as_float().abs())),
        "__float_floor" => BuiltinResult::Value(Value::Float(args[0].as_float().floor())),
        "__float_ceil" => BuiltinResult::Value(Value::Float(args[0].as_float().ceil())),
        "__float_round" => BuiltinResult::Value(Value::Float(args[0].as_float().round())),
        "__float_min" => {
            let (a, b) = two_floats(args);
            BuiltinResult::Value(Value::Float(a.min(b)))
        }
        "__float_max" => {
            let (a, b) = two_floats(args);
            BuiltinResult::Value(Value::Float(a.max(b)))
        }
        "__float_to_string" => {
            let s = format_float(args[0].as_float());
            BuiltinResult::Value(Value::Str(s.into()))
        }
        "__float_parse" => {
            let s = args[0].as_str();
            let parsed = s.parse::<f64>();
            set_errno(if parsed.is_ok() { 0 } else { 1 });
            BuiltinResult::Value(Value::Float(parsed.unwrap_or(0.0)))
        }
        // Called with no args — reads errno set by __float_parse.
        "__float_parse_errno" => {
            BuiltinResult::Value(Value::Int(get_errno()))
        }
        "__float_from_int" => BuiltinResult::Value(Value::Float(args[0].as_int() as f64)),
        "__float_to_int" => BuiltinResult::Value(Value::Int(args[0].as_float() as i64)),
        "__float_is_nan" => BuiltinResult::Value(Value::Bool(args[0].as_float().is_nan())),
        "__float_is_infinite" => BuiltinResult::Value(Value::Bool(args[0].as_float().is_infinite())),

        // ── String ──────────────────────────────────────────────────────────
        "__string_len" => {
            let s = args[0].as_str();
            BuiltinResult::Value(Value::Int(s.chars().count() as i64))
        }
        "__string_is_empty" => {
            BuiltinResult::Value(Value::Bool(args[0].as_str().is_empty()))
        }
        "__string_trim" => {
            let s = args[0].as_str();
            BuiltinResult::Value(Value::Str(s.trim().into()))
        }
        "__string_to_upper" => {
            let s = args[0].as_str();
            let r: String = s.chars().map(|c| if c.is_ascii() { c.to_ascii_uppercase() } else { c }).collect();
            BuiltinResult::Value(Value::Str(r.into()))
        }
        "__string_to_lower" => {
            let s = args[0].as_str();
            let r: String = s.chars().map(|c| if c.is_ascii() { c.to_ascii_lowercase() } else { c }).collect();
            BuiltinResult::Value(Value::Str(r.into()))
        }
        "__string_contains" => {
            let s = args[0].as_str();
            let needle = args[1].as_str();
            BuiltinResult::Value(Value::Bool(s.contains(needle.as_ref())))
        }
        "__string_starts_with" => {
            let s = args[0].as_str();
            let prefix = args[1].as_str();
            BuiltinResult::Value(Value::Bool(s.starts_with(prefix.as_ref())))
        }
        "__string_ends_with" => {
            let s = args[0].as_str();
            let suffix = args[1].as_str();
            BuiltinResult::Value(Value::Bool(s.ends_with(suffix.as_ref())))
        }
        "__string_parse_int" => {
            let s = args[0].as_str();
            let parsed = s.parse::<i64>();
            set_errno(if parsed.is_ok() { 0 } else { 1 });
            BuiltinResult::Value(Value::Int(parsed.unwrap_or(0)))
        }
        // Called with no args — reads errno set by __string_parse_int.
        "__string_parse_errno" => {
            BuiltinResult::Value(Value::Int(get_errno()))
        }
        "__string_byte_at" => {
            let s = args[0].as_str();
            let idx = args[1].as_int() as usize;
            let byte = s.as_bytes().get(idx).copied().unwrap_or(0);
            BuiltinResult::Value(Value::Int(byte as i64))
        }
        "__string_char_at" => {
            let s = args[0].as_str();
            let idx = args[1].as_int() as usize;
            let ch: Option<char> = s.chars().nth(idx);
            set_errno(if ch.is_some() { 0 } else { 1 });
            let result = ch.map(|c| c.to_string()).unwrap_or_default();
            BuiltinResult::Value(Value::Str(result.into()))
        }
        // Called with no args — reads errno set by the preceding __string_char_at
        // or __string_from_char_code call.
        "__string_char_errno" => {
            BuiltinResult::Value(Value::Int(get_errno()))
        }
        "__string_char_code" => {
            let s = args[0].as_str();
            let code = s.chars().next().map(|c| c as i64).unwrap_or(0);
            BuiltinResult::Value(Value::Int(code))
        }
        "__string_from_char_code" => {
            let code = args[0].as_int() as u32;
            // Reject surrogates (0xD800..=0xDFFF) and values above 0x10FFFF.
            let ch = char::from_u32(code);
            let is_surrogate = (0xD800..=0xDFFF).contains(&code);
            set_errno(if ch.is_some() && !is_surrogate { 0 } else { 1 });
            let result = ch.filter(|_| !is_surrogate).map(|c| c.to_string()).unwrap_or_default();
            BuiltinResult::Value(Value::Str(result.into()))
        }
        "__string_substring" => {
            let s = args[0].as_str();
            let start = args[1].as_int() as usize;
            let len = args[2].as_int() as usize;
            let sub: String = s.chars().skip(start).take(len).collect();
            BuiltinResult::Value(Value::Str(sub.into()))
        }
        "__string_reverse" => {
            let s = args[0].as_str();
            let rev: String = s.chars().rev().collect();
            BuiltinResult::Value(Value::Str(rev.into()))
        }
        "__string_from_byte" => {
            let b = args[0].as_int() as u8;
            let s = std::str::from_utf8(&[b]).unwrap_or("").to_string();
            BuiltinResult::Value(Value::Str(s.into()))
        }
        "__string_replace" => {
            let s = args[0].as_str();
            let from = args[1].as_str();
            let to = args[2].as_str();
            let result = s.replace(from.as_ref(), to.as_ref());
            BuiltinResult::Value(Value::Str(result.into()))
        }
        "__string_split_whitespace" => {
            let s = args[0].as_str();
            let parts: Vec<Value> = s.split_whitespace().map(|p| Value::Str(p.into())).collect();
            BuiltinResult::Value(Value::List(Rc::new(RefCell::new(parts))))
        }
        "__string_split" => {
            let s = args[0].as_str();
            let delim = args[1].as_str();
            let parts: Vec<Value> = s.split(delim.as_ref()).map(|p| Value::Str(p.into())).collect();
            BuiltinResult::Value(Value::List(Rc::new(RefCell::new(parts))))
        }
        "__string_chars" => {
            let s = args[0].as_str();
            let chars: Vec<Value> = s.chars().map(|c| Value::Str(c.to_string().into())).collect();
            BuiltinResult::Value(Value::List(Rc::new(RefCell::new(chars))))
        }
        "__string_join" => {
            let list = match &args[0] {
                Value::List(rc) => rc.borrow().clone(),
                _ => panic!("interpreter: __string_join: expected List"),
            };
            let sep = args[1].as_str();
            let strs: Vec<String> = list.iter().map(|v| match v {
                Value::Str(s) => s.as_ref().to_string(),
                _ => panic!("interpreter: __string_join: expected List<String>"),
            }).collect();
            BuiltinResult::Value(Value::Str(strs.join(sep.as_ref()).into()))
        }

        // ── List ────────────────────────────────────────────────────────────
        "__list_sort" => {
            let list = list_clone(&args[0], "__list_sort");
            let mut sorted = list;
            sorted.sort_by(|a, b| a.as_int().cmp(&b.as_int()));
            BuiltinResult::Value(Value::List(Rc::new(RefCell::new(sorted))))
        }
        "__list_sort_str" => {
            let list = list_clone(&args[0], "__list_sort_str");
            let mut sorted = list;
            sorted.sort_by(|a, b| a.as_str().as_ref().cmp(b.as_str().as_ref()));
            BuiltinResult::Value(Value::List(Rc::new(RefCell::new(sorted))))
        }
        "__list_int_push" => {
            let mut list = list_clone(&args[0], "__list_int_push");
            list.push(args[1].clone());
            BuiltinResult::Value(Value::List(Rc::new(RefCell::new(list))))
        }
        "__list_int_sum" => {
            let list = list_clone(&args[0], "__list_int_sum");
            let sum: i64 = list.iter().map(|v| v.as_int()).sum();
            BuiltinResult::Value(Value::Int(sum))
        }
        "__list_int_contains" => {
            let list = list_clone(&args[0], "__list_int_contains");
            let needle = args[1].as_int();
            BuiltinResult::Value(Value::Bool(list.iter().any(|v| v.as_int() == needle)))
        }
        "__list_int_index_of" => {
            let list = list_clone(&args[0], "__list_int_index_of");
            let needle = args[1].as_int();
            let idx = list.iter().position(|v| v.as_int() == needle)
                .map(|i| i as i64)
                .unwrap_or(-1);
            BuiltinResult::Value(Value::Int(idx))
        }
        "__list_int_max" => {
            let list = list_clone(&args[0], "__list_int_max");
            let max = list.iter().map(|v| v.as_int()).max().unwrap_or(i64::MIN);
            BuiltinResult::Value(Value::Int(max))
        }
        "__list_int_min" => {
            let list = list_clone(&args[0], "__list_int_min");
            let min = list.iter().map(|v| v.as_int()).min().unwrap_or(i64::MAX);
            BuiltinResult::Value(Value::Int(min))
        }

        // ── Int parse ────────────────────────────────────────────────────────
        "parse__Int" => {
            let s = args[0].as_str();
            let n: i64 = s.parse().unwrap_or(0);
            BuiltinResult::Value(Value::Int(n))
        }

        // ── sys.args ─────────────────────────────────────────────────────────
        "sys__args" => {
            BuiltinResult::Value(Value::List(Rc::new(RefCell::new(vec![]))))
        }

        // ── Option/Result display ────────────────────────────────────────────
        // __display_option__T(tag, payload) — MIR emits AdtTag + AdtPayload then calls this.
        // tag=0 → Some(payload), tag=1 → None. Mirror of runtime/src/stdlib_display.rs.
        name if name.starts_with("__display_option__") => {
            let tag = args[0].as_int();
            let s = if tag == 0 {
                format!("Some({})", format_value_display(&args[1]))
            } else {
                "None".to_string()
            };
            BuiltinResult::Value(Value::Str(s.into()))
        }
        // __display_result__T(res) — if ever called (not yet in corpus), same 2-arg pattern.
        name if name.starts_with("__display_result__") => {
            let res = &args[0];
            let s = format_result(res);
            BuiltinResult::Value(Value::Str(s.into()))
        }

        // ── JSON (handle-based) ───────────────────────────────────────────────
        // Mirrors runtime/src/stdlib_json.rs. stdlib/json.ty calls __json_parse
        // which returns an Int handle (0 = error). All other __json_* intrinsics
        // take a handle and return typed results. Handle 0 = absent / error.
        "__json_parse" => {
            let s = args[0].as_str();
            match json_parse_node(s.as_ref()) {
                Some(h) => BuiltinResult::Value(Value::Int(h)),
                None => {
                    json_set_err("parse error", 0, 0);
                    BuiltinResult::Value(Value::Int(0))
                }
            }
        }
        "__json_err_msg" => {
            let msg = JSON_ERR.with(|e| e.borrow().0.clone());
            BuiltinResult::Value(Value::Str(msg.into()))
        }
        "__json_err_line" => {
            BuiltinResult::Value(Value::Int(JSON_ERR.with(|e| e.borrow().1)))
        }
        "__json_err_col" => {
            BuiltinResult::Value(Value::Int(JSON_ERR.with(|e| e.borrow().2)))
        }
        "__json_kind" => {
            let h = args[0].as_int();
            let kind: &str = match json_lookup(h) {
                Some(JsonNode::Null)     => "null",
                Some(JsonNode::Bool(_))  => "bool",
                Some(JsonNode::Int(_))   => "int",
                Some(JsonNode::Str(_))   => "string",
                Some(JsonNode::Array(_)) => "array",
                Some(JsonNode::Object(_))=> "object",
                None                     => "null",
            };
            BuiltinResult::Value(Value::Str(kind.into()))
        }
        "__json_is_string" => {
            let h = args[0].as_int();
            BuiltinResult::Value(Value::Bool(matches!(json_lookup(h), Some(JsonNode::Str(_)))))
        }
        "__json_str" => {
            let h = args[0].as_int();
            let s = match json_lookup(h) { Some(JsonNode::Str(s)) => s, _ => Rc::from("") };
            BuiltinResult::Value(Value::Str(s))
        }
        "__json_is_int" => {
            let h = args[0].as_int();
            BuiltinResult::Value(Value::Bool(matches!(json_lookup(h), Some(JsonNode::Int(_)))))
        }
        "__json_int" => {
            let h = args[0].as_int();
            let n = match json_lookup(h) { Some(JsonNode::Int(n)) => n, _ => 0 };
            BuiltinResult::Value(Value::Int(n))
        }
        "__json_is_bool" => {
            let h = args[0].as_int();
            BuiltinResult::Value(Value::Bool(matches!(json_lookup(h), Some(JsonNode::Bool(_)))))
        }
        "__json_bool" => {
            let h = args[0].as_int();
            let b = match json_lookup(h) { Some(JsonNode::Bool(b)) => b, _ => false };
            BuiltinResult::Value(Value::Bool(b))
        }
        "__json_get" => {
            let h = args[0].as_int();
            let key = args[1].as_str();
            let child_h = match json_lookup(h) {
                Some(JsonNode::Object(entries)) => entries.iter()
                    .find(|(k, _)| k.as_ref() == key.as_ref())
                    .map(|(_, h)| *h)
                    .unwrap_or(0),
                _ => 0,
            };
            BuiltinResult::Value(Value::Int(child_h))
        }
        "__json_at" => {
            let h = args[0].as_int();
            let idx = args[1].as_int();
            let child_h = match json_lookup(h) {
                Some(JsonNode::Array(elems)) if idx >= 0 => {
                    elems.get(idx as usize).copied().unwrap_or(0)
                }
                _ => 0,
            };
            BuiltinResult::Value(Value::Int(child_h))
        }

        // ── Unsupported ──────────────────────────────────────────────────────
        name if is_unsupported_builtin(name) => {
            panic!(
                "interpreter: unsupported builtin '{}' (not available in Playground or --interpret mode)",
                name
            )
        }

        _ => BuiltinResult::NotABuiltin,
    }
}

fn is_unsupported_builtin(name: &str) -> bool {
    name.starts_with("__fs_")
        || name.starts_with("__io_")
        || name.starts_with("__http_")
        || name.starts_with("__time_")
        || name.starts_with("__log_")
        || name.starts_with("__bench_")
}

fn format_option(v: &Value) -> String {
    match v {
        Value::Struct { fields, .. } => {
            let tag = fields.first().map(|t| t.as_int()).unwrap_or(1);
            if tag == 0 {
                // Some(payload)
                let payload = fields.get(1).map(|p| format_value_display(p)).unwrap_or_default();
                format!("Some({})", payload)
            } else {
                "None".to_string()
            }
        }
        _ => format!("{:?}", v),
    }
}

fn format_result(v: &Value) -> String {
    match v {
        Value::Struct { fields, .. } => {
            let tag = fields.first().map(|t| t.as_int()).unwrap_or(1);
            if tag == 0 {
                let payload = fields.get(1).map(|p| format_value_display(p)).unwrap_or_default();
                format!("Ok({})", payload)
            } else {
                let payload = fields.get(1).map(|p| format_value_display(p)).unwrap_or_default();
                format!("Err({})", payload)
            }
        }
        _ => format!("{:?}", v),
    }
}

fn format_value_display(v: &Value) -> String {
    match v {
        Value::Str(s) => s.as_ref().to_string(),
        Value::Int(n) => n.to_string(),
        Value::Float(f) => format_float(*f),
        Value::Bool(b) => if *b { "true".to_string() } else { "false".to_string() },
        Value::Unit => "()".to_string(),
        Value::Struct { fields, type_name } if type_name.as_ref().starts_with("Option") => {
            format_option(v)
        }
        Value::Struct { fields, type_name } if type_name.as_ref().starts_with("Result") => {
            format_result(v)
        }
        _ => format!("{:?}", v),
    }
}

// ── JSON helpers (handle-based) ───────────────────────────────────────────────
// Mirrors runtime/src/stdlib_json.rs. json.ty stdlib calls __json_parse which
// returns an Int handle (0 = absent). Nodes are stored in a thread-local table.

#[derive(Clone, Debug)]
enum JsonNode {
    Null,
    Bool(bool),
    Int(i64),
    Str(Rc<str>),
    Array(Vec<i64>),             // element handles
    Object(Vec<(Rc<str>, i64)>), // key → child handle
}

thread_local! {
    // Index 0 = sentinel (absent). Nodes start at index 1.
    static JSON_TABLE: RefCell<Vec<JsonNode>> = RefCell::new(vec![JsonNode::Null]);
    // (message, line, col)
    static JSON_ERR: RefCell<(String, i64, i64)> = RefCell::new((String::new(), 0, 0));
}

fn json_alloc(node: JsonNode) -> i64 {
    JSON_TABLE.with(|t| {
        let mut tbl = t.borrow_mut();
        let h = tbl.len() as i64;
        tbl.push(node);
        h
    })
}

fn json_lookup(h: i64) -> Option<JsonNode> {
    if h == 0 { return None; }
    JSON_TABLE.with(|t| t.borrow().get(h as usize).cloned())
}

fn json_set_err(msg: &str, line: i64, col: i64) {
    JSON_ERR.with(|e| *e.borrow_mut() = (msg.to_string(), line, col));
}

fn json_parse_node(s: &str) -> Option<i64> {
    let s = s.trim();
    if s == "null"  { return Some(json_alloc(JsonNode::Null)); }
    if s == "true"  { return Some(json_alloc(JsonNode::Bool(true))); }
    if s == "false" { return Some(json_alloc(JsonNode::Bool(false))); }
    if let Ok(n) = s.parse::<i64>() { return Some(json_alloc(JsonNode::Int(n))); }
    if s.starts_with('"') && s.ends_with('"') && s.len() >= 2 {
        let decoded = json_unescape(&s[1..s.len()-1]);
        return Some(json_alloc(JsonNode::Str(decoded.into())));
    }
    if s.starts_with('[') && s.ends_with(']') {
        let elems = json_split_top(&s[1..s.len()-1])?;
        let mut handles = Vec::new();
        for e in &elems { handles.push(json_parse_node(e.trim())?); }
        return Some(json_alloc(JsonNode::Array(handles)));
    }
    if s.starts_with('{') && s.ends_with('}') {
        let pairs = json_split_top(&s[1..s.len()-1])?;
        let mut entries: Vec<(Rc<str>, i64)> = Vec::new();
        for pair in &pairs {
            let colon = json_find_colon(pair)?;
            let ks = pair[..colon].trim();
            let vs = pair[colon+1..].trim();
            let key: Rc<str> = if ks.starts_with('"') && ks.ends_with('"') && ks.len() >= 2 {
                json_unescape(&ks[1..ks.len()-1]).into()
            } else { return None; };
            entries.push((key, json_parse_node(vs)?));
        }
        return Some(json_alloc(JsonNode::Object(entries)));
    }
    None
}

fn json_unescape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\\' {
            match chars.next() {
                Some('"')  => out.push('"'),
                Some('\\') => out.push('\\'),
                Some('/')  => out.push('/'),
                Some('n')  => out.push('\n'),
                Some('r')  => out.push('\r'),
                Some('t')  => out.push('\t'),
                Some('u')  => {
                    let hex: String = (0..4).filter_map(|_| chars.next()).collect();
                    if let Ok(n) = u32::from_str_radix(&hex, 16) {
                        if let Some(ch) = char::from_u32(n) {
                            out.push(ch);
                        }
                    }
                }
                Some(other) => { out.push('\\'); out.push(other); }
                None => {}
            }
        } else {
            out.push(c);
        }
    }
    out
}

fn json_split_top(s: &str) -> Option<Vec<String>> {
    let mut parts = Vec::new();
    let mut depth = 0i32;
    let mut in_str = false;
    let mut escape = false;
    let mut start = 0;
    for (i, c) in s.char_indices() {
        if escape { escape = false; continue; }
        if c == '\\' && in_str { escape = true; continue; }
        if c == '"' { in_str = !in_str; continue; }
        if in_str { continue; }
        match c {
            '[' | '{' => depth += 1,
            ']' | '}' => depth -= 1,
            ',' if depth == 0 => {
                parts.push(s[start..i].to_string());
                start = i + 1;
            }
            _ => {}
        }
    }
    if in_str { return None; }
    parts.push(s[start..].to_string());
    Some(parts.into_iter().filter(|p| !p.trim().is_empty()).collect())
}

fn json_find_colon(s: &str) -> Option<usize> {
    let mut in_str = false;
    let mut escape = false;
    for (i, c) in s.char_indices() {
        if escape { escape = false; continue; }
        if c == '\\' && in_str { escape = true; continue; }
        if c == '"' { in_str = !in_str; continue; }
        if !in_str && c == ':' { return Some(i); }
    }
    None
}

fn list_clone(v: &Value, ctx: &str) -> Vec<Value> {
    match v {
        Value::List(rc) => rc.borrow().clone(),
        _ => panic!("interpreter: {}: expected List, got {:?}", ctx, v),
    }
}

/// Display a value as the print family would show it.
pub fn value_to_display(v: &Value) -> String {
    match v {
        Value::Str(s) => s.as_ref().to_string(),
        Value::Int(n) => n.to_string(),
        Value::Float(f) => format_float(*f),
        // LLVM lowers Bool to i64 (1/0), so #{bool_expr} prints "1" or "0".
        Value::Bool(b) => if *b { "1".into() } else { "0".into() },
        Value::Unit => String::new(),
        _ => format!("{:?}", v),
    }
}

fn two_floats(args: Vec<Value>) -> (f64, f64) {
    let mut it = args.into_iter();
    let a = it.next().unwrap().as_float();
    let b = it.next().unwrap().as_float();
    (a, b)
}

// ── Collection builtins ──────────────────────────────────────────────────────

/// Dispatch collection builtins (map/set/linked_map/sorted_map etc.).
pub fn call_collection_builtin(fname: &str, args: Vec<Value>) -> Option<BuiltinResult> {
    let (fam, body) = if let Some(b) = fname.strip_prefix("__sorted_map_") {
        ("sorted_map", b)
    } else if let Some(b) = fname.strip_prefix("__sorted_set_") {
        ("sorted_set", b)
    } else if let Some(b) = fname.strip_prefix("__linked_map_") {
        ("linked_map", b)
    } else if let Some(b) = fname.strip_prefix("__linked_set_") {
        ("linked_set", b)
    } else if let Some(b) = fname.strip_prefix("__map_") {
        ("map", b)
    } else if let Some(b) = fname.strip_prefix("__set_") {
        ("set", b)
    } else {
        return None;
    };

    let is_map = fam.contains("map");
    let is_sorted = fam.starts_with("sorted");

    let (op, _rest) = body.split_once("__").unwrap_or((body, ""));

    let result = match op {
        "new" => {
            if is_map {
                if is_sorted {
                    Value::SortedMap(Rc::new(RefCell::new(vec![])))
                } else {
                    Value::Map(Rc::new(RefCell::new(vec![])))
                }
            } else if is_sorted {
                Value::SortedSet(Rc::new(RefCell::new(vec![])))
            } else {
                Value::Set(Rc::new(RefCell::new(vec![])))
            }
        }
        "len" => {
            let n = match &args[0] {
                Value::Map(rc) => rc.borrow().len(),
                Value::SortedMap(rc) => rc.borrow().len(),
                Value::Set(rc) => rc.borrow().len(),
                Value::SortedSet(rc) => rc.borrow().len(),
                _ => panic!("interpreter: {}: expected collection, got {:?}", fname, args[0]),
            };
            Value::Int(n as i64)
        }
        "insert" => {
            if is_map {
                let key = args[1].clone();
                let val = args[2].clone();
                match &args[0] {
                    Value::Map(rc) => {
                        let mut m = rc.borrow_mut();
                        if let Some(entry) = m.iter_mut().find(|(k, _)| key_eq(k, &key)) {
                            entry.1 = val;
                        } else {
                            m.push((key, val));
                        }
                        Value::Map(rc.clone())
                    }
                    Value::SortedMap(rc) => {
                        let mut m = rc.borrow_mut();
                        if let Some(pos) = m.iter().position(|(k, _)| key_eq(k, &key)) {
                            m[pos].1 = val;
                        } else {
                            let ins = m.partition_point(|(k, _)| {
                                key_cmp(k, &key) == std::cmp::Ordering::Less
                            });
                            m.insert(ins, (key, val));
                        }
                        Value::SortedMap(rc.clone())
                    }
                    _ => panic!("interpreter: {}: expected Map/SortedMap", fname),
                }
            } else {
                let elem = args[1].clone();
                match &args[0] {
                    Value::Set(rc) => {
                        let mut s = rc.borrow_mut();
                        if !s.iter().any(|v| key_eq(v, &elem)) {
                            s.push(elem);
                        }
                        Value::Set(rc.clone())
                    }
                    Value::SortedSet(rc) => {
                        let mut s = rc.borrow_mut();
                        if !s.iter().any(|v| key_eq(v, &elem)) {
                            let ins = s.partition_point(|v| {
                                key_cmp(v, &elem) == std::cmp::Ordering::Less
                            });
                            s.insert(ins, elem);
                        }
                        Value::SortedSet(rc.clone())
                    }
                    _ => panic!("interpreter: {}: expected Set/SortedSet", fname),
                }
            }
        }
        "remove" => {
            let key = args[1].clone();
            match &args[0] {
                Value::Map(rc) => {
                    rc.borrow_mut().retain(|(k, _)| !key_eq(k, &key));
                    Value::Map(rc.clone())
                }
                Value::SortedMap(rc) => {
                    rc.borrow_mut().retain(|(k, _)| !key_eq(k, &key));
                    Value::SortedMap(rc.clone())
                }
                Value::Set(rc) => {
                    rc.borrow_mut().retain(|v| !key_eq(v, &key));
                    Value::Set(rc.clone())
                }
                Value::SortedSet(rc) => {
                    rc.borrow_mut().retain(|v| !key_eq(v, &key));
                    Value::SortedSet(rc.clone())
                }
                _ => panic!("interpreter: {}: expected collection", fname),
            }
        }
        "contains" => {
            let key = &args[1];
            let found = match &args[0] {
                Value::Map(rc) => rc.borrow().iter().any(|(k, _)| key_eq(k, key)),
                Value::SortedMap(rc) => rc.borrow().iter().any(|(k, _)| key_eq(k, key)),
                Value::Set(rc) => rc.borrow().iter().any(|v| key_eq(v, key)),
                Value::SortedSet(rc) => rc.borrow().iter().any(|v| key_eq(v, key)),
                _ => panic!("interpreter: {}: expected collection", fname),
            };
            Value::Bool(found)
        }
        _ => panic!("interpreter: unsupported collection op '{}' in '{}'", op, fname),
    };

    Some(BuiltinResult::Value(result))
}
