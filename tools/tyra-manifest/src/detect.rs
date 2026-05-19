/// Determine whether a Tyra source file is a *bin* (entry-point) file.
///
/// A file is a bin if its top-level scope contains `fn main` or any
/// executable statement (as defined by ADR 0006 and ADR 0009).
///
/// This is a lightweight line-level heuristic — it does not run the full
/// parser. It is used by `tyra mod sync` to reject bin packages before they
/// are written to the cache. The full compiler performs the same check at
/// compile time as a second line of defense.
///
/// **Heuristic rules** (applied to lines with no leading whitespace):
/// 1. A line starting with `fn main` (possibly followed by `(`) → bin.
/// 2. A line that is non-empty, not a comment (`#`), and does not begin with a
///    declaration keyword (`fn`, `type`, `value`, `data`, `trait`, `impl`,
///    `import`, `export`) or a structural keyword (`end`, `when`) → treated as
///    a top-level executable statement → bin.
///
/// False positives (incorrectly calling a lib a bin) cause `E_DEP_NOT_IMPORTABLE`
/// at sync time, surfacing the problem early. False negatives (missing a bin)
/// are caught at compile time by `resolve_imports`.
pub fn is_bin_source(src: &str) -> bool {
    const DECLARATION_PREFIXES: &[&str] = &[
        "fn ", "fn(", "type ", "value ", "data ", "trait ", "impl ",
        "import ", "export ", "end", "when ",
    ];

    for line in src.lines() {
        // Only examine lines at column 0 (no leading whitespace).
        if line.starts_with(' ') || line.starts_with('\t') {
            continue;
        }
        let trimmed = line.trim_end();
        if trimmed.is_empty() {
            continue;
        }
        // Any line starting with '#' is a comment regardless of what follows.
        if trimmed.starts_with('#') {
            continue;
        }
        // `fn main` or `export fn main` is a bin entry point only when `main`
        // is not part of a longer identifier (e.g. `fn mainframe` is a lib fn).
        // export fn main(...) is still a bin — export does not change the
        // entry-point semantics (ADR 0009).
        for prefix in &["fn main", "export fn main"] {
            if let Some(after) = trimmed.strip_prefix(prefix) {
                let boundary = after
                    .chars()
                    .next()
                    .map(|c| c == '(' || c == ' ' || c == '\t')
                    .unwrap_or(true);
                if boundary {
                    return true;
                }
            }
        }
        // If the line is not a declaration or structural keyword, it is an
        // executable statement at top level.
        if !DECLARATION_PREFIXES.iter().any(|p| trimmed.starts_with(p)) {
            return true;
        }
    }
    false
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fn_main_is_bin() {
        assert!(is_bin_source("fn main() -> Unit\n  ()\nend\n"));
    }

    #[test]
    fn top_level_statement_is_bin() {
        assert!(is_bin_source("print(\"hello\\n\")\n"));
    }

    #[test]
    fn let_binding_at_top_level_is_bin() {
        assert!(is_bin_source("let x = 1\nprint(\"#{x}\\n\")\n"));
    }

    #[test]
    fn declarations_only_is_lib() {
        let src = "\
export fn greet(name: String) -> String
  \"hello, #{name}\"
end
";
        assert!(!is_bin_source(src));
    }

    #[test]
    fn import_and_fn_without_main_is_lib() {
        let src = "\
import string

export fn shout(s: String) -> String
  string.to_upper(s)
end
";
        assert!(!is_bin_source(src));
    }

    #[test]
    fn comment_only_is_lib() {
        assert!(!is_bin_source("# just a comment\n"));
    }

    #[test]
    fn empty_source_is_lib() {
        assert!(!is_bin_source(""));
    }

    #[test]
    fn type_alias_is_lib() {
        assert!(!is_bin_source("type UserId = Int\n"));
    }

    #[test]
    fn data_decl_is_lib() {
        let src = "\
data Point
  x: Float
  y: Float
end
";
        assert!(!is_bin_source(src));
    }

    // Regression: fn mainframe / fn main_loop must NOT be treated as bin.
    #[test]
    fn fn_mainframe_is_lib() {
        let src = "\
export fn mainframe(args: List<String>) -> Unit
  ()
end
";
        assert!(!is_bin_source(src), "fn mainframe must not be treated as bin");
    }

    #[test]
    fn fn_main_loop_is_lib() {
        let src = "\
fn main_loop(state: State) -> State
  state
end
";
        assert!(!is_bin_source(src), "fn main_loop must not be treated as bin");
    }

    // Regression: #comment (no space after #) must be treated as a comment.
    #[test]
    fn comment_without_space_is_lib() {
        let src = "\
#todo: add more exports
export fn greet(name: String) -> String
  \"hello, #{name}\"
end
";
        assert!(!is_bin_source(src), "#todo comment must not be treated as executable statement");
    }

    #[test]
    fn comment_hashbang_style_is_lib() {
        let src = "\
#!tyra
export fn run() -> Unit
  ()
end
";
        assert!(!is_bin_source(src), "#! line must not be treated as executable statement");
    }

    // Regression: export fn main(...) must be treated as bin (ADR 0009).
    #[test]
    fn export_fn_main_is_bin() {
        let src = "\
export fn main() -> Unit
  print(\"hello\\n\")
end
";
        assert!(is_bin_source(src), "export fn main must be treated as bin");
    }

    // export fn mainframe must still be lib.
    #[test]
    fn export_fn_mainframe_is_lib() {
        let src = "\
export fn mainframe(args: List<String>) -> Unit
  ()
end
";
        assert!(!is_bin_source(src), "export fn mainframe must not be treated as bin");
    }

    // fn main with no parens on the same line is still bin.
    #[test]
    fn fn_main_bare_is_bin() {
        assert!(is_bin_source("fn main() -> Unit\n  ()\nend\n"));
    }

    // fn main followed immediately by end-of-string is bin.
    #[test]
    fn fn_main_eof_is_bin() {
        assert!(is_bin_source("fn main"));
    }
}
