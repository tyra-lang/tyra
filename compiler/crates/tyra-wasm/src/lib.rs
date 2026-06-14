//! tyra-wasm: wasm-bindgen wrapper for the Tyra Playground.
//!
//! Exposes a single `run(source)` function that compiles and interprets Tyra
//! source in the browser without any server-side execution or LLVM.
//!
//! stdlib modules bundled (via include_str!):
//!   string, list, float, json, map, set, linked_map, linked_set,
//!   sorted_map, sorted_set, assert
//!
//! NOT bundled (→ E0200 on use, which is the correct Playground behavior):
//!   io, fs, http, log, time  (stdin/network/filesystem are not available
//!   in a browser sandbox; surfacing E0200 tells the user clearly)
//!
//! core.sys / core.tasks are handled by the frontend (builtin modules);
//! they never reach the loader.

use tyra_frontend::{ImportLoader, ImportResolution};
use tyra_interpreter::RunOutcome;
use wasm_bindgen::prelude::*;

// ── Bundled stdlib sources ────────────────────────────────────────────────────

macro_rules! bundle {
    ($name:literal, $file:literal) => {
        ($name, include_str!(concat!("../../../../stdlib/", $file)))
    };
}

const BUNDLED: &[(&str, &str)] = &[
    bundle!("string",     "string.ty"),
    bundle!("list",       "list.ty"),
    bundle!("float",      "float.ty"),
    bundle!("json",       "json.ty"),
    bundle!("map",        "map.ty"),
    bundle!("set",        "set.ty"),
    bundle!("linked_map", "linked_map.ty"),
    bundle!("linked_set", "linked_set.ty"),
    bundle!("sorted_map", "sorted_map.ty"),
    bundle!("sorted_set", "sorted_set.ty"),
    bundle!("assert",     "assert.ty"),
];

// ── BundledImportLoader ───────────────────────────────────────────────────────

struct BundledImportLoader;

impl ImportLoader for BundledImportLoader {
    fn resolve(&self, path: &[String]) -> ImportResolution {
        if path.len() != 1 {
            return ImportResolution::NotFound;
        }
        let name = path[0].as_str();
        for &(bundled_name, src) in BUNDLED {
            if bundled_name == name {
                return ImportResolution::Found {
                    display_name: format!("<bundled:{name}>"),
                    source: src.to_string(),
                };
            }
        }
        ImportResolution::NotFound
    }
}

// ── JSON helpers ──────────────────────────────────────────────────────────────

fn esc(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '"'  => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c    => out.push(c),
        }
    }
    out
}

fn build_response(outcome: Option<RunOutcome>, diag_json: String) -> String {
    let (stdout, stderr, exit_code) = match outcome {
        Some(o) => (o.stdout, o.stderr, o.exit_code),
        None    => (String::new(), String::new(), 1),
    };
    format!(
        "{{\"stdout\":\"{}\",\"stderr\":\"{}\",\"exit_code\":{},\"diagnostics\":{}}}",
        esc(&stdout),
        esc(&stderr),
        exit_code,
        diag_json,
    )
}

fn diagnostics_to_json(
    report: &tyra_frontend::Report,
    sources: &tyra_frontend::SourceMap,
) -> String {
    let mut out = String::from("[");
    for (i, diag) in report.diagnostics().iter().enumerate() {
        if i > 0 { out.push(','); }
        out.push_str("{\"severity\":\"");
        out.push_str(diag.level.as_str());
        out.push('"');
        if let Some(code) = &diag.code {
            out.push_str(",\"code\":\"");
            out.push_str(&esc(code));
            out.push('"');
        }
        out.push_str(",\"message\":\"");
        out.push_str(&esc(&diag.message));
        out.push_str("\",\"spans\":[");
        for (j, label) in diag.labels.iter().enumerate() {
            if j > 0 { out.push(','); }
            let (l, c) = sources.line_col(label.span.source, label.span.start);
            let (el, ec) = sources.line_col(label.span.source, label.span.end);
            out.push_str(&format!(
                "{{\"line\":{l},\"col\":{c},\"end_line\":{el},\"end_col\":{ec},\"label\":\"{}\"}}",
                esc(&label.message)
            ));
        }
        out.push(']');
        if let Some(help) = &diag.help {
            out.push_str(",\"help\":\"");
            out.push_str(&esc(help));
            out.push('"');
        }
        if !diag.notes.is_empty() {
            out.push_str(",\"notes\":[");
            for (k, note) in diag.notes.iter().enumerate() {
                if k > 0 { out.push(','); }
                out.push('"');
                out.push_str(&esc(note));
                out.push('"');
            }
            out.push(']');
        }
        out.push('}');
    }
    out.push(']');
    out
}

// ── Public WASM API ───────────────────────────────────────────────────────────

/// Compile and interpret a Tyra source string, returning a JSON string:
/// `{"stdout":"...","stderr":"...","exit_code":0,"diagnostics":[...]}`
///
/// JS usage: `const r = JSON.parse(tyra_wasm.run(src))`
///
/// On compile errors: exit_code=1, diagnostics non-empty, stdout/stderr empty.
/// On user-visible runtime errors (execution limit exceeded, unsupported async,
/// integer division by zero): stderr captures the message, exit_code=101.
/// Internal interpreter bugs (type-mismatch on already type-checked IR) may still
/// propagate as WASM traps; these represent interpreter bugs, not user errors.
#[wasm_bindgen]
pub fn run(source: &str) -> String {
    let loader = BundledImportLoader;
    let fr = tyra_frontend::compile_unit("playground.ty", source, &loader);
    let diag_json = diagnostics_to_json(&fr.check.report, &fr.check.sources);

    if fr.check.report.has_errors() {
        return build_response(None, diag_json);
    }

    let program = match fr.program {
        Some(p) => p,
        None    => return build_response(None, diag_json),
    };

    let outcome = tyra_interpreter::run(&program);
    build_response(Some(outcome), diag_json)
}

/// Return a JSON array of bundled stdlib module names (for Playground UI hints).
#[wasm_bindgen]
pub fn bundled_modules() -> String {
    let names: Vec<String> = BUNDLED.iter().map(|(n, _)| format!("\"{}\"", n)).collect();
    format!("[{}]", names.join(","))
}
