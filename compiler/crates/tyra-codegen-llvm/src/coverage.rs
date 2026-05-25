// Coverage instrumentation helpers (ADR 0014 §coverage, Phase 3c).
//
// Tyra-native line/function hit counter approach (not LLVM covmap-compatible).
// Counter array lives as a global in the emitted IR; the runtime atexit handler
// flushes it to `$TYRA_COV_DIR/<pid>.covraw` (little-endian i64 array).
// A companion `<binary>.tyra-covmap` sidecar (text format, written at compile
// time by the driver) maps each counter index to a source location.

use std::collections::HashMap;
use std::fmt::Write as FmtWrite;

use tyra_mir::{Function, Instruction, Program, SourceLoc};

/// Counter-to-source mapping, built once before IR emission.
pub struct CovMap {
    /// Total number of counters (= length of the runtime counter array).
    pub n: u32,
    /// `(file_id, line)` → counter index (shared across BBs on the same line).
    pub counter_for: HashMap<(u32, u32), u32>,
    /// Function name → the counter index of that function's entry block.
    pub fn_entry_ctr: HashMap<String, u32>,
    /// All entries in stable order for covmap file writing.
    pub entries: Vec<CovEntry>,
}

/// One entry in the covmap sidecar file.
pub struct CovEntry {
    pub counter_idx: u32,
    pub file_id: u32,
    pub line: u32,
    /// `Some(fn_name)` if this counter is the entry point of a function.
    pub fn_name: Option<String>,
}

/// Build the CovMap by scanning all functions in the program.
///
/// For each function:
///   - The implicit entry block gets a counter using the first non-dummy loc.
///   - Each explicit `Instruction::Label` gets a counter using the Label
///     stmt's loc, or the next stmt's loc if the Label itself is dummy.
///
/// Multiple BBs mapping to the same `(file_id, line)` share one counter.
pub fn build_cov_map(program: &Program) -> CovMap {
    let mut counter_for: HashMap<(u32, u32), u32> = HashMap::new();
    let mut fn_entry_ctr: HashMap<String, u32> = HashMap::new();
    let mut entries: Vec<CovEntry> = Vec::new();
    let mut next_idx: u32 = 0;

    for func in &program.functions {
        // Function entry block: use first non-dummy loc in body.
        if let Some(loc) = first_non_dummy_loc(func) {
            let idx = get_or_assign(
                loc,
                Some(&func.name),
                &mut counter_for,
                &mut entries,
                &mut next_idx,
            );
            fn_entry_ctr.insert(func.name.clone(), idx);
        }

        // Explicit Label BBs.
        for (i, stmt) in func.body.iter().enumerate() {
            if matches!(&stmt.instr, Instruction::Label(_)) {
                let loc = if !stmt.loc.is_dummy() {
                    Some(stmt.loc)
                } else {
                    func.body[i + 1..]
                        .iter()
                        .find(|s| !s.loc.is_dummy())
                        .map(|s| s.loc)
                };
                if let Some(loc) = loc {
                    get_or_assign(loc, None, &mut counter_for, &mut entries, &mut next_idx);
                }
            }
        }
    }

    CovMap {
        n: next_idx,
        counter_for,
        fn_entry_ctr,
        entries,
    }
}

fn get_or_assign(
    loc: SourceLoc,
    fn_name: Option<&str>,
    counter_for: &mut HashMap<(u32, u32), u32>,
    entries: &mut Vec<CovEntry>,
    next_idx: &mut u32,
) -> u32 {
    let key = (loc.file_id, loc.line);
    if let Some(&idx) = counter_for.get(&key) {
        // Propagate fn_name to an existing entry that has none.
        if fn_name.is_some() && entries[idx as usize].fn_name.is_none() {
            entries[idx as usize].fn_name = fn_name.map(|s| s.to_owned());
        }
        return idx;
    }
    let idx = *next_idx;
    *next_idx += 1;
    counter_for.insert(key, idx);
    entries.push(CovEntry {
        counter_idx: idx,
        file_id: loc.file_id,
        line: loc.line,
        fn_name: fn_name.map(|s| s.to_owned()),
    });
    idx
}

fn first_non_dummy_loc(func: &Function) -> Option<SourceLoc> {
    func.body.iter().find(|s| !s.loc.is_dummy()).map(|s| s.loc)
}

/// Emit `@.tyra_counters = global [N x i64] zeroinitializer` into `out`.
pub fn emit_counter_global(out: &mut String, n: u32) {
    let real_n = if n == 0 { 1 } else { n };
    writeln!(
        out,
        "@.tyra_counters = global [{real_n} x i64] zeroinitializer"
    )
    .unwrap();
}

/// Emit the `tyra_cov_init` extern declaration.
pub fn emit_cov_extern(out: &mut String) {
    writeln!(out, "declare void @tyra_cov_init(ptr, i64)").unwrap();
}

/// Emit a counter increment for the given source location into `out`.
/// `cov_id` is a per-function incrementing suffix to avoid SSA name collisions.
pub fn emit_cov_increment(out: &mut String, loc: SourceLoc, cov_map: &CovMap, cov_id: &mut u32) {
    if loc.is_dummy() {
        return;
    }
    let key = (loc.file_id, loc.line);
    if let Some(&idx) = cov_map.counter_for.get(&key) {
        let id = *cov_id;
        *cov_id += 1;
        let real_n = if cov_map.n == 0 { 1 } else { cov_map.n };
        writeln!(
            out,
            "  %__cov_gep_{id} = getelementptr [{real_n} x i64], ptr @.tyra_counters, i64 0, i64 {idx}"
        )
        .unwrap();
        writeln!(
            out,
            "  %__cov_old_{id} = atomicrmw add ptr %__cov_gep_{id}, i64 1 monotonic"
        )
        .unwrap();
    }
}

/// Emit the `tyra_cov_init(...)` call (placed in main after GC_init/tyra_rt_init).
pub fn emit_cov_init_call(out: &mut String, n: u32) {
    let real_n = if n == 0 { 1u32 } else { n };
    writeln!(
        out,
        "  call void @tyra_cov_init(ptr @.tyra_counters, i64 {real_n})"
    )
    .unwrap();
}

// ── Covmap file format ────────────────────────────────────────────────────────

/// Serialize the CovMap to the covmap sidecar text format.
///
/// ```text
/// TYRA_COVMAP_V1
/// N_COUNTERS=<n>
/// N_FILES=<k>
/// FILE:0=<path>
/// ...
/// N_ENTRIES=<m>
/// CTR:<idx>:<file_id>:<line>[:<fn_name>]
/// ...
/// ```
pub fn write_covmap_text(cov_map: &CovMap, source_files: &[String]) -> String {
    let mut out = String::new();
    writeln!(out, "TYRA_COVMAP_V1").unwrap();
    writeln!(out, "N_COUNTERS={}", cov_map.n).unwrap();
    writeln!(out, "N_FILES={}", source_files.len()).unwrap();
    for (i, path) in source_files.iter().enumerate() {
        writeln!(out, "FILE:{i}={path}").unwrap();
    }
    writeln!(out, "N_ENTRIES={}", cov_map.entries.len()).unwrap();
    for e in &cov_map.entries {
        if let Some(fn_name) = &e.fn_name {
            writeln!(
                out,
                "CTR:{}:{}:{}:{}",
                e.counter_idx, e.file_id, e.line, fn_name
            )
            .unwrap();
        } else {
            writeln!(out, "CTR:{}:{}:{}", e.counter_idx, e.file_id, e.line).unwrap();
        }
    }
    out
}

// ── Covmap parsing ────────────────────────────────────────────────────────────

#[derive(Debug)]
pub struct ParsedCovMap {
    pub n: u32,
    pub files: Vec<String>,
    pub entries: Vec<ParsedEntry>,
}

#[derive(Debug)]
pub struct ParsedEntry {
    pub counter_idx: u32,
    pub file_id: u32,
    pub line: u32,
    pub fn_name: Option<String>,
}

pub fn parse_covmap(text: &str) -> Option<ParsedCovMap> {
    let mut lines = text.lines();
    if lines.next()? != "TYRA_COVMAP_V1" {
        return None;
    }
    let n: u32 = lines.next()?.strip_prefix("N_COUNTERS=")?.parse().ok()?;
    let n_files: u32 = lines.next()?.strip_prefix("N_FILES=")?.parse().ok()?;
    let mut files = Vec::new();
    for _ in 0..n_files {
        let line = lines.next()?;
        let eq = line.find('=')?;
        files.push(line[eq + 1..].to_owned());
    }
    let n_entries: u32 = lines.next()?.strip_prefix("N_ENTRIES=")?.parse().ok()?;
    let mut entries = Vec::new();
    for _ in 0..n_entries {
        let line = lines.next()?;
        let rest = line.strip_prefix("CTR:")?;
        let parts: Vec<&str> = rest.splitn(4, ':').collect();
        if parts.len() < 3 {
            return None;
        }
        entries.push(ParsedEntry {
            counter_idx: parts[0].parse().ok()?,
            file_id: parts[1].parse().ok()?,
            line: parts[2].parse().ok()?,
            fn_name: if parts.len() == 4 {
                Some(parts[3].to_owned())
            } else {
                None
            },
        });
    }
    Some(ParsedCovMap { n, files, entries })
}

// ── Covraw merging ────────────────────────────────────────────────────────────

/// Merge all `*.covraw` files in `covraw_dir` element-wise.
/// Returns None if no valid covraw files are found.
pub fn merge_covraw(covraw_dir: &std::path::Path, n: u32) -> Option<Vec<i64>> {
    let mut merged: Vec<i64> = vec![0i64; n as usize];
    let mut found_any = false;

    let dir_entries = std::fs::read_dir(covraw_dir).ok()?;
    for entry in dir_entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("covraw") {
            continue;
        }
        let bytes = match std::fs::read(&path) {
            Ok(b) => b,
            Err(_) => continue,
        };
        if bytes.len() != n as usize * 8 {
            continue;
        }
        found_any = true;
        for (i, chunk) in bytes.chunks_exact(8).enumerate() {
            let v = i64::from_le_bytes(chunk.try_into().unwrap());
            merged[i] = merged[i].saturating_add(v);
        }
    }

    if found_any { Some(merged) } else { None }
}

// ── Report formatting ─────────────────────────────────────────────────────────

/// Generate a human-readable coverage report.
pub fn format_report(cov_map: &ParsedCovMap, counters: &[i64]) -> String {
    let mut by_file: HashMap<u32, Vec<&ParsedEntry>> = HashMap::new();
    for e in &cov_map.entries {
        by_file.entry(e.file_id).or_default().push(e);
    }

    let mut total_lines = 0u32;
    let mut covered_lines = 0u32;
    let mut total_fns = 0u32;
    let mut covered_fns = 0u32;
    let mut report = String::new();

    let mut file_ids: Vec<u32> = by_file.keys().copied().collect();
    file_ids.sort();

    for file_id in file_ids {
        let entries = &by_file[&file_id];
        let file_path = cov_map
            .files
            .get(file_id as usize)
            .map(|s| s.as_str())
            .unwrap_or("?");

        let mut seen_lines: HashMap<u32, bool> = HashMap::new();
        let mut fn_hits: HashMap<String, bool> = HashMap::new();

        for e in entries.iter().copied() {
            let hit =
                (e.counter_idx as usize) < counters.len() && counters[e.counter_idx as usize] > 0;
            seen_lines
                .entry(e.line)
                .and_modify(|v| *v = *v || hit)
                .or_insert(hit);
            if let Some(fn_name) = &e.fn_name {
                fn_hits
                    .entry(fn_name.clone())
                    .and_modify(|v| *v = *v || hit)
                    .or_insert(hit);
            }
        }

        let file_total_l = seen_lines.len() as u32;
        let file_covered_l = seen_lines.values().filter(|&&h| h).count() as u32;
        let file_total_f = fn_hits.len() as u32;
        let file_covered_f = fn_hits.values().filter(|&&h| h).count() as u32;

        total_lines += file_total_l;
        covered_lines += file_covered_l;
        total_fns += file_total_f;
        covered_fns += file_covered_f;

        let l_pct = pct(file_covered_l, file_total_l);
        let f_pct = pct(file_covered_f, file_total_f);
        writeln!(
            report,
            "{file_path}: lines {file_covered_l}/{file_total_l} ({l_pct:.1}%) | fns {file_covered_f}/{file_total_f} ({f_pct:.1}%)"
        )
        .unwrap();
    }

    let tl_pct = pct(covered_lines, total_lines);
    let tf_pct = pct(covered_fns, total_fns);
    writeln!(report).unwrap();
    writeln!(
        report,
        "TOTAL: lines {covered_lines}/{total_lines} ({tl_pct:.1}%) | fns {covered_fns}/{total_fns} ({tf_pct:.1}%)"
    )
    .unwrap();
    writeln!(report, "(branch coverage: not reported — see ADR 0014)").unwrap();
    report
}

fn pct(covered: u32, total: u32) -> f64 {
    if total == 0 {
        100.0
    } else {
        100.0 * covered as f64 / total as f64
    }
}
