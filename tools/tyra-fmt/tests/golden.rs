// Golden idempotency tests for tyra-fmt.
//
// For each .tyra file in bench/static-corpus (excluding bad/ error cases),
// verify that:
//   1. fmt_source succeeds (no parse errors)
//   2. Formatting is idempotent: fmt(fmt(src)) == fmt(src)

use std::path::Path;
use tyra_fmt::fmt_source;

fn corpus_dir() -> &'static Path {
    Path::new(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../bench/static-corpus"
    ))
}

fn assert_idempotent(path: &Path) {
    let src = std::fs::read_to_string(path)
        .unwrap_or_else(|e| panic!("cannot read {}: {e}", path.display()));
    let first = fmt_source(&src)
        .unwrap_or_else(|e| panic!("{}: fmt failed: {e}", path.display()));
    let second = fmt_source(&first)
        .unwrap_or_else(|e| panic!("{}: re-fmt failed: {e}", path.display()));
    assert_eq!(
        first, second,
        "{}: output is not idempotent",
        path.display()
    );
}

#[test]
fn golden_static_corpus() {
    let dir = corpus_dir();
    let mut count = 0;
    for entry in std::fs::read_dir(dir).expect("cannot read corpus dir") {
        let entry = entry.unwrap();
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("tyra") {
            continue;
        }
        assert_idempotent(&path);
        count += 1;
    }
    assert!(count > 0, "no .tyra files found in static-corpus");
}

