//! tyra-pkg: dependency management commands for the Tyra language.
//!
//! Public API:
//! - `run_init(dest, name)` — create Tyra.toml in an existing directory
//! - `run_add(project_root, dep_name, source)` — append a dependency entry
//! - `run_tree(project_root)` — render the dependency tree as a string
//! - `run_sync(project_root)` — clone git deps into `~/.tyra/cache/git/`
//! - `run_clean()` — remove the entire `~/.tyra/cache/` directory
//! - `tyra_cache_root()` — path to the Tyra cache root (`~/.tyra/cache/`)
//! - `cache_dir_for(dep_name, url, rev)` — canonical cache path for a git dep

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::process::Command;
use tyra_manifest::{Dependency, find_project_root, load_manifest};

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

// Spec §5.2 reserved words — dep names must not collide with these.
const RESERVED_WORDS: &[&str] = &[
    "fn", "data", "value", "type", "trait", "impl", "let", "mut", "if", "else", "match",
    "when", "for", "in", "while", "return", "defer", "async", "await", "spawn", "import",
    "export", "and", "or", "not", "true", "false", "end",
];

// ---------------------------------------------------------------------------
// Public types
// ---------------------------------------------------------------------------

#[derive(Debug)]
pub enum PkgError {
    Io(std::io::Error),
    Manifest(tyra_manifest::ManifestError),
    /// `Tyra.toml` already exists (`tyra mod init` on an existing project).
    AlreadyExists(PathBuf),
    /// No `Tyra.toml` found walking up from cwd.
    NoProject,
    /// Dependency with this name is already declared.
    DuplicateDep(String),
    /// Dependency with this name is not declared.
    DepNotFound(String),
    /// Package or dep name violates naming rules.
    InvalidName(String),
    /// `git` binary not found in PATH.
    GitNotAvailable,
    /// Git clone or checkout failed for a dependency.
    SyncFailed { dep: String, message: String },
    /// Dependency root is a bin package (ADR 0009 E_DEP_NOT_IMPORTABLE).
    BinDepNotImportable(String),
    /// Dependency key does not match the package name declared in `Tyra.toml`.
    NameMismatch { key: String, package_name: String },
    /// Root module `src/<name>.tyra` is absent (ADR 0009 requires it).
    MissingRootModule(String),
}

impl std::fmt::Display for PkgError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PkgError::Io(e) => write!(f, "I/O error: {e}"),
            PkgError::Manifest(e) => write!(f, "{e}"),
            PkgError::AlreadyExists(p) => {
                write!(f, "Tyra.toml already exists: {}", p.display())
            }
            PkgError::NoProject => write!(
                f,
                "no Tyra.toml found in the current directory or any parent"
            ),
            PkgError::DuplicateDep(n) => {
                write!(f, "dependency `{n}` is already declared in [dependencies]")
            }
            PkgError::DepNotFound(n) => {
                write!(f, "dependency `{n}` is not declared in [dependencies]")
            }
            PkgError::InvalidName(n) => write!(
                f,
                "invalid name `{n}`: must start with a lowercase letter, \
                 contain only lowercase letters, digits, and underscores, \
                 and must not be a reserved word"
            ),
            PkgError::GitNotAvailable => write!(
                f,
                "`git` not found in PATH; install Git to sync git dependencies"
            ),
            PkgError::SyncFailed { dep, message } => {
                write!(f, "sync failed for dependency `{dep}`: {message}")
            }
            PkgError::BinDepNotImportable(dep) => write!(
                f,
                "dependency `{dep}` is a bin package and cannot be imported \
                 (ADR 0009 E_DEP_NOT_IMPORTABLE)"
            ),
            PkgError::NameMismatch { key, package_name } => write!(
                f,
                "dependency key `{key}` does not match the package name `{package_name}`; \
                 the dependency key must equal the package name"
            ),
            PkgError::MissingRootModule(dep) => write!(
                f,
                "dependency `{dep}` has no root module `src/{dep}.tyra` (ADR 0009 requires it)"
            ),
        }
    }
}

impl std::error::Error for PkgError {}

impl From<std::io::Error> for PkgError {
    fn from(e: std::io::Error) -> Self {
        PkgError::Io(e)
    }
}

impl From<tyra_manifest::ManifestError> for PkgError {
    fn from(e: tyra_manifest::ManifestError) -> Self {
        PkgError::Manifest(e)
    }
}

// ---------------------------------------------------------------------------
// Dependency source
// ---------------------------------------------------------------------------

pub enum DepSource {
    Path(String),
    Git { url: String, rev: String },
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// `tyra mod init [--name <name>]`
///
/// Creates `Tyra.toml` in `dest`. If `name` is `None`, the directory name is
/// used as the package name.
pub fn run_init(dest: &Path, name: Option<&str>) -> Result<(), PkgError> {
    let manifest_path = dest.join("Tyra.toml");
    if manifest_path.exists() {
        return Err(PkgError::AlreadyExists(manifest_path));
    }
    let pkg_name = match name {
        Some(n) => n.to_string(),
        None => dest
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("unnamed")
            .to_string(),
    };
    validate_name(&pkg_name)?;
    let content = format!(
        "[package]\nname    = \"{pkg_name}\"\nversion = \"0.1.0\"\nedition = \"2026\"\n"
    );
    std::fs::write(&manifest_path, content)?;
    Ok(())
}

/// `tyra mod add <dep_name> --path <path>` / `--git <url> --rev <rev>`
///
/// Appends a new `[dependencies]` entry. Creates the section header if absent.
pub fn run_add(project_root: &Path, dep_name: &str, source: DepSource) -> Result<(), PkgError> {
    validate_name(dep_name)?;
    let manifest = load_manifest(project_root)?;
    if manifest.dependencies.contains_key(dep_name) {
        return Err(PkgError::DuplicateDep(dep_name.to_string()));
    }
    // For path deps: validate that the dep key matches the package name
    // declared in the dependency's own Tyra.toml (if accessible).
    if let DepSource::Path(rel) = &source {
        let dep_root = project_root.join(rel);
        if let Ok(dep_manifest) = load_manifest(&dep_root) {
            if dep_manifest.package.name != dep_name {
                return Err(PkgError::NameMismatch {
                    key: dep_name.to_string(),
                    package_name: dep_manifest.package.name.clone(),
                });
            }
        }
    }

    let new_line = match &source {
        DepSource::Path(p) => format!("{dep_name} = {{ path = \"{p}\" }}"),
        DepSource::Git { url, rev } => {
            format!("{dep_name} = {{ git = \"{url}\", rev = \"{rev}\" }}")
        }
    };
    let manifest_path = project_root.join("Tyra.toml");
    let content = std::fs::read_to_string(&manifest_path)?;
    let updated = insert_dependency_line(&content, &new_line);
    std::fs::write(&manifest_path, updated)?;
    Ok(())
}

/// `tyra mod tree`
///
/// Returns the dependency tree rooted at `project_root` as a formatted string.
/// Git dependencies are shown as `[not synced]`; path deps are resolved
/// recursively. Cycles are detected and labelled `[cycle]`.
pub fn run_tree(project_root: &Path) -> Result<String, PkgError> {
    let manifest = load_manifest(project_root)?;
    let mut out = String::new();
    out.push_str(&format!("{} {}\n", manifest.package.name, manifest.package.version));

    let mut visited = HashSet::new();
    let canonical =
        project_root.canonicalize().unwrap_or_else(|_| project_root.to_path_buf());
    visited.insert(canonical);

    let mut deps: Vec<(&String, &Dependency)> = manifest.dependencies.iter().collect();
    deps.sort_by_key(|(k, _)| k.as_str());

    let count = deps.len();
    for (i, (name, dep)) in deps.iter().enumerate() {
        print_dep(&mut out, name, dep, project_root, "", i == count - 1, &mut visited);
    }
    Ok(out)
}

/// Locate the project root walking up from `start`, then call `run_tree`.
pub fn run_tree_from(start: &Path) -> Result<String, PkgError> {
    let root = find_project_root(start).ok_or(PkgError::NoProject)?;
    run_tree(&root)
}

/// `tyra mod tree --json`
///
/// Returns the dependency tree as a JSON string preserving the full recursive
/// structure. Each node is:
/// `{"key":"<dep-key>","name":"<pkg>","version":"<v>","source":"<path|git>","deps":[...]}`
/// Root node omits `key` and `source`.
pub fn run_tree_json(project_root: &Path) -> Result<String, PkgError> {
    let manifest = load_manifest(project_root)?;
    let mut visited = HashSet::new();
    let canonical = project_root.canonicalize().unwrap_or_else(|_| project_root.to_path_buf());
    visited.insert(canonical);

    let mut deps_json: Vec<String> = Vec::new();
    let mut deps: Vec<(&String, &Dependency)> = manifest.dependencies.iter().collect();
    deps.sort_by_key(|(k, _)| k.as_str());
    for (name, dep) in &deps {
        deps_json.push(dep_node_json(name, dep, project_root, &mut visited));
    }

    Ok(format!(
        "{{\"name\":{},\"version\":{},\"deps\":[{}]}}\n",
        json_str(&manifest.package.name),
        json_str(&manifest.package.version),
        deps_json.join(",")
    ))
}

/// Locate the project root walking up from `start`, then call `run_tree_json`.
pub fn run_tree_json_from(start: &Path) -> Result<String, PkgError> {
    let root = find_project_root(start).ok_or(PkgError::NoProject)?;
    run_tree_json(&root)
}

/// Locate the project root walking up from `start`, then call `run_add`.
pub fn run_add_from(
    start: &Path,
    dep_name: &str,
    source: DepSource,
) -> Result<(), PkgError> {
    let root = find_project_root(start).ok_or(PkgError::NoProject)?;
    run_add(&root, dep_name, source)
}

/// `tyra mod remove <dep_name>`
///
/// Removes the named dependency entry from `[dependencies]` in `Tyra.toml`.
pub fn run_remove(project_root: &Path, dep_name: &str) -> Result<(), PkgError> {
    let manifest = load_manifest(project_root)?;
    if !manifest.dependencies.contains_key(dep_name) {
        return Err(PkgError::DepNotFound(dep_name.to_string()));
    }
    let manifest_path = project_root.join("Tyra.toml");
    let content = std::fs::read_to_string(&manifest_path)?;
    let updated = remove_dependency_line(&content, dep_name);
    std::fs::write(&manifest_path, updated)?;
    Ok(())
}

/// Locate the project root walking up from `start`, then call `run_remove`.
pub fn run_remove_from(start: &Path, dep_name: &str) -> Result<(), PkgError> {
    let root = find_project_root(start).ok_or(PkgError::NoProject)?;
    run_remove(&root, dep_name)
}

/// `tyra mod sync`
///
/// Clones all git dependencies declared in `project_root/Tyra.toml` into
/// `~/.tyra/cache/git/<dep_name>/<rev>/`.  Path dependencies are skipped.
pub fn run_sync(project_root: &Path) -> Result<SyncReport, PkgError> {
    let manifest = load_manifest(project_root)?;
    let mut report = SyncReport::default();

    let mut deps: Vec<(&String, &Dependency)> = manifest.dependencies.iter().collect();
    deps.sort_by_key(|(k, _)| k.as_str());

    for (dep_name, dep) in &deps {
        match (&dep.path, &dep.git, &dep.rev) {
            (Some(_), _, _) => {
                report.skipped.push(dep_name.to_string());
            }
            (None, Some(url), Some(rev)) => {
                match sync_git_dep(dep_name, url, rev)? {
                    SyncStatus::Fresh => report.synced.push(dep_name.to_string()),
                    SyncStatus::Cached => report.cached.push(dep_name.to_string()),
                }
            }
            _ => {} // load_manifest already validated
        }
    }
    Ok(report)
}

/// Locate the project root walking up from `start`, then call `run_sync`.
pub fn run_sync_from(start: &Path) -> Result<SyncReport, PkgError> {
    let root = find_project_root(start).ok_or(PkgError::NoProject)?;
    run_sync(&root)
}

/// `tyra mod show <dep_name>`
///
/// Returns a human-readable summary of a single dependency entry.
pub fn run_show(project_root: &Path, dep_name: &str) -> Result<String, PkgError> {
    let manifest = load_manifest(project_root)?;
    let dep = manifest
        .dependencies
        .get(dep_name)
        .ok_or_else(|| PkgError::DepNotFound(dep_name.to_string()))?;

    let mut out = format!("{dep_name}\n");

    if let Some(path_str) = &dep.path {
        let abs = project_root.join(path_str);
        out.push_str(&format!("  source:  path {path_str}\n"));
        out.push_str(&format!("  root:    {}\n", abs.display()));
        match load_manifest(&abs) {
            Ok(m) => {
                out.push_str(&format!("  name:    {}\n", m.package.name));
                out.push_str(&format!("  version: {}\n", m.package.version));
            }
            Err(_) => {
                out.push_str("  (manifest not readable)\n");
            }
        }
    } else if let Some(url) = &dep.git {
        let rev = dep.rev.as_deref().unwrap_or("?");
        out.push_str(&format!("  source:  git {url}\n"));
        out.push_str(&format!("  rev:     {rev}\n"));
        let cache = cache_dir_for(dep_name, url, rev);
        let synced = cache.join("Tyra.toml").is_file();
        out.push_str(&format!("  cache:   {}\n", cache.display()));
        out.push_str(&format!("  synced:  {}\n", if synced { "yes" } else { "no" }));
    }

    Ok(out)
}

/// Locate the project root walking up from `start`, then call `run_show`.
pub fn run_show_from(start: &Path, dep_name: &str) -> Result<String, PkgError> {
    let root = find_project_root(start).ok_or(PkgError::NoProject)?;
    run_show(&root, dep_name)
}

/// Returns the root of the Tyra local cache: `~/.tyra/cache/`.
pub fn tyra_cache_root() -> PathBuf {
    let home = std::env::var("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("."));
    home.join(".tyra").join("cache")
}

/// `tyra mod clean`
///
/// Removes the entire `~/.tyra/cache/` directory.  Returns `true` if the
/// directory existed and was removed, `false` if it was already absent.
pub fn run_clean() -> Result<bool, PkgError> {
    let root = tyra_cache_root();
    if root.exists() {
        std::fs::remove_dir_all(&root)?;
        Ok(true)
    } else {
        Ok(false)
    }
}

/// `tyra mod sync --check`
///
/// Validates all dependencies without mutating the cache:
/// - path deps: load manifest and check invariants
/// - git deps: verify cache entry exists and passes `validate_dep_root`
///
/// Returns a list of issues found (empty = all good).
pub fn run_sync_check(project_root: &Path) -> Result<Vec<String>, PkgError> {
    let manifest = load_manifest(project_root)?;
    let mut issues: Vec<String> = Vec::new();

    let mut deps: Vec<(&String, &Dependency)> = manifest.dependencies.iter().collect();
    deps.sort_by_key(|(k, _)| k.as_str());

    for (dep_name, dep) in &deps {
        match (&dep.path, &dep.git, &dep.rev) {
            (Some(rel), _, _) => {
                let dep_root = project_root.join(rel);
                if let Err(e) = validate_dep_root(dep_name, &dep_root) {
                    issues.push(format!("{dep_name}: {e}"));
                }
            }
            (None, Some(url), Some(rev)) => {
                let cache_dir = cache_dir_for(dep_name, url, rev);
                if !cache_dir.join("Tyra.toml").is_file() {
                    issues.push(format!("{dep_name}: not synced (run `tyra mod sync`)"));
                } else if let Err(e) = validate_dep_root(dep_name, &cache_dir) {
                    issues.push(format!("{dep_name}: {e}"));
                }
            }
            _ => {}
        }
    }
    Ok(issues)
}

/// Locate the project root walking up from `start`, then call `run_sync_check`.
pub fn run_sync_check_from(start: &Path) -> Result<Vec<String>, PkgError> {
    let root = find_project_root(start).ok_or(PkgError::NoProject)?;
    run_sync_check(&root)
}

/// Canonical cache directory for a git dependency.
///
/// `~/.tyra/cache/git/<dep_name>-<url_hash12>/<rev>/`
///
/// The URL hash prevents cache collisions when two manifests declare the same
/// dependency name pointing to different repositories.
pub fn cache_dir_for(dep_name: &str, url: &str, rev: &str) -> PathBuf {
    let home = std::env::var("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("."));
    let dir_name = format!("{dep_name}-{}", url_hash(url));
    home.join(".tyra").join("cache").join("git").join(dir_name).join(rev)
}

/// 12-character lowercase hex of FNV-1a(url). No extra crate needed.
fn url_hash(url: &str) -> String {
    let mut h: u64 = 0xcbf29ce484222325;
    for b in url.bytes() {
        h ^= b as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    format!("{:012x}", h & 0x0000_ffff_ffff_ffff)
}

/// Report returned by `run_sync`.
#[derive(Debug, Default)]
pub struct SyncReport {
    pub synced: Vec<String>,
    pub cached: Vec<String>,
    pub skipped: Vec<String>,
}

impl std::fmt::Display for SyncReport {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        for name in &self.synced {
            writeln!(f, "  synced  {name}")?;
        }
        for name in &self.cached {
            writeln!(f, "  cached  {name}")?;
        }
        for name in &self.skipped {
            writeln!(f, "  skipped {name} (path)")?;
        }
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

enum SyncStatus {
    Fresh,
    Cached,
}

/// Validate ADR 0009/0010 invariants for an already-populated dependency root:
/// - `Tyra.toml` must be loadable
/// - `package.name` must equal `dep_name` (no aliasing)
/// - root module must not be a bin package
///
/// Used by both the fresh-clone path and the cache-hit path so that stale or
/// manually-populated caches cannot bypass the checks.
pub(crate) fn validate_dep_root(dep_name: &str, dep_root: &Path) -> Result<(), PkgError> {
    let dep_manifest = load_manifest(dep_root).map_err(|e| PkgError::SyncFailed {
        dep: dep_name.to_string(),
        message: format!("invalid Tyra.toml in dependency: {e}"),
    })?;
    if dep_manifest.package.name != dep_name {
        return Err(PkgError::NameMismatch {
            key: dep_name.to_string(),
            package_name: dep_manifest.package.name.clone(),
        });
    }
    // ADR 0009: root module src/<name>.tyra must exist; its absence means the
    // dependency is unusable (import would fail with E0200) and is caught here.
    let root_src = dep_root
        .join("src")
        .join(format!("{}.tyra", dep_manifest.package.name));
    if !root_src.is_file() {
        return Err(PkgError::MissingRootModule(dep_name.to_string()));
    }
    let src = std::fs::read_to_string(&root_src).unwrap_or_default();
    if tyra_manifest::is_bin_source(&src) {
        return Err(PkgError::BinDepNotImportable(dep_name.to_string()));
    }
    Ok(())
}

fn sync_git_dep(dep_name: &str, url: &str, rev: &str) -> Result<SyncStatus, PkgError> {
    let cache_dir = cache_dir_for(dep_name, url, rev);

    // Already in cache? Re-validate ADR 0009/0010 invariants even for cached
    // entries — stale or manually-populated caches must still satisfy them.
    if cache_dir.join("Tyra.toml").is_file() {
        validate_dep_root(dep_name, &cache_dir)?;
        return Ok(SyncStatus::Cached);
    }

    // Ensure the parent directory exists.
    let cache_parent = cache_dir.parent().unwrap_or(&cache_dir);
    std::fs::create_dir_all(cache_parent)?;

    // Use a tmp directory adjacent to the final cache dir so that rename is
    // within the same filesystem (atomic on POSIX).
    let safe_rev = rev.replace(['/', '\\', ':'], "-");
    let tmp_dir = cache_parent.join(format!("_tmp_{dep_name}_{safe_rev}"));
    if tmp_dir.exists() {
        std::fs::remove_dir_all(&tmp_dir)?;
    }

    // git clone <url> <tmp_dir>
    let clone_status = Command::new("git")
        .args(["clone", url, tmp_dir.to_str().unwrap_or(".")])
        .status()
        .map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                PkgError::GitNotAvailable
            } else {
                PkgError::Io(e)
            }
        })?;
    if !clone_status.success() {
        let _ = std::fs::remove_dir_all(&tmp_dir);
        return Err(PkgError::SyncFailed {
            dep: dep_name.to_string(),
            message: format!("git clone failed for `{url}`"),
        });
    }

    // git -C <tmp_dir> checkout <rev>
    let checkout_status = Command::new("git")
        .args(["-C", tmp_dir.to_str().unwrap_or("."), "checkout", rev])
        .status()
        .map_err(|e| PkgError::Io(e))?;
    if !checkout_status.success() {
        let _ = std::fs::remove_dir_all(&tmp_dir);
        return Err(PkgError::SyncFailed {
            dep: dep_name.to_string(),
            message: format!("git checkout `{rev}` failed"),
        });
    }

    // Validate ADR 0009/0010 invariants before committing to cache.
    validate_dep_root(dep_name, &tmp_dir).map_err(|e| {
        let _ = std::fs::remove_dir_all(&tmp_dir);
        e
    })?;

    // Atomic rename into the cache.
    std::fs::rename(&tmp_dir, &cache_dir).map_err(|e| {
        let _ = std::fs::remove_dir_all(&tmp_dir);
        PkgError::Io(e)
    })?;

    Ok(SyncStatus::Fresh)
}

fn validate_name(name: &str) -> Result<(), PkgError> {
    let ok = !name.is_empty()
        && name.chars().next().map(|c| c.is_ascii_lowercase()).unwrap_or(false)
        && name.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
        && !RESERVED_WORDS.contains(&name);
    if ok {
        Ok(())
    } else {
        Err(PkgError::InvalidName(name.to_string()))
    }
}

/// Insert `new_line` into the `[dependencies]` section of a `Tyra.toml`
/// string. Creates the section if absent.
fn insert_dependency_line(content: &str, new_line: &str) -> String {
    let lines: Vec<&str> = content.lines().collect();
    let trailing_newline = content.ends_with('\n');

    let dep_header_idx = lines.iter().position(|l| l.trim() == "[dependencies]");

    let mut result: Vec<String> = lines.iter().map(|s| s.to_string()).collect();

    if let Some(header_idx) = dep_header_idx {
        // Find where the section ends (next bare [section] header or EOF).
        let section_end = lines[header_idx + 1..]
            .iter()
            .position(|l| {
                let t = l.trim();
                !t.is_empty() && t.starts_with('[') && !t.starts_with("[[")
            })
            .map(|i| header_idx + 1 + i)
            .unwrap_or(lines.len());

        // Insert after the last non-empty line within the section.
        let insert_after = lines[header_idx..section_end]
            .iter()
            .rposition(|l| !l.trim().is_empty())
            .map(|i| header_idx + i)
            .unwrap_or(header_idx);

        result.insert(insert_after + 1, new_line.to_string());
    } else {
        result.push(String::new());
        result.push("[dependencies]".to_string());
        result.push(new_line.to_string());
    }

    let mut s = result.join("\n");
    if trailing_newline {
        s.push('\n');
    }
    s
}

/// Remove the line for `dep_name` from the `[dependencies]` section of a
/// `Tyra.toml` string. Leaves the section header intact even if the section
/// becomes empty.
fn remove_dependency_line(content: &str, dep_name: &str) -> String {
    let trailing_newline = content.ends_with('\n');
    let result: Vec<&str> = content
        .lines()
        .filter(|l| {
            let t = l.trim_start();
            // Drop lines that start with `dep_name` followed by `=` or whitespace+`=`.
            !(t.starts_with(dep_name)
                && t[dep_name.len()..].trim_start().starts_with('='))
        })
        .collect();
    let mut s = result.join("\n");
    if trailing_newline {
        s.push('\n');
    }
    s
}

/// Escape a string for JSON output.
fn json_str(s: &str) -> String {
    format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\""))
}

/// Recursively build a JSON object for one dependency node.
fn dep_node_json(
    key: &str,
    dep: &Dependency,
    parent_root: &Path,
    visited: &mut HashSet<PathBuf>,
) -> String {
    if let Some(path_str) = &dep.path {
        let dep_root = parent_root.join(path_str);
        let canonical = dep_root.canonicalize().unwrap_or_else(|_| dep_root.clone());
        if visited.contains(&canonical) {
            return format!(
                "{{\"key\":{},\"source\":{},\"cycle\":true}}",
                json_str(key),
                json_str(&format!("path:{path_str}"))
            );
        }
        match load_manifest(&dep_root) {
            Ok(m) => {
                visited.insert(canonical.clone());
                let mut child_deps: Vec<(&String, &Dependency)> = m.dependencies.iter().collect();
                child_deps.sort_by_key(|(k, _)| k.as_str());
                let children: Vec<String> = child_deps
                    .iter()
                    .map(|(k, d)| dep_node_json(k, d, &dep_root, visited))
                    .collect();
                visited.remove(&canonical);
                format!(
                    "{{\"key\":{},\"name\":{},\"version\":{},\"source\":{},\"deps\":[{}]}}",
                    json_str(key),
                    json_str(&m.package.name),
                    json_str(&m.package.version),
                    json_str(&format!("path:{path_str}")),
                    children.join(",")
                )
            }
            Err(e) => format!(
                "{{\"key\":{},\"source\":{},\"error\":{}}}",
                json_str(key),
                json_str(&format!("path:{path_str}")),
                json_str(&e.to_string())
            ),
        }
    } else if let Some(url) = &dep.git {
        let rev = dep.rev.as_deref().unwrap_or("?");
        format!(
            "{{\"key\":{},\"source\":{},\"rev\":{},\"synced\":false}}",
            json_str(key),
            json_str(&format!("git:{url}")),
            json_str(rev)
        )
    } else {
        format!("{{\"key\":{}}}", json_str(key))
    }
}

/// Recursively render one dependency node into `out`.
fn print_dep(
    out: &mut String,
    name: &str,
    dep: &Dependency,
    parent_root: &Path,
    prefix: &str,
    is_last: bool,
    visited: &mut HashSet<PathBuf>,
) {
    let connector = if is_last { "└── " } else { "├── " };
    let child_prefix = format!("{prefix}{}", if is_last { "    " } else { "│   " });

    if let Some(path_str) = &dep.path {
        let dep_root = parent_root.join(path_str);
        let canonical = dep_root.canonicalize().unwrap_or_else(|_| dep_root.clone());

        if visited.contains(&canonical) {
            out.push_str(&format!(
                "{prefix}{connector}{name} (path: {path_str}) [cycle]\n"
            ));
            return;
        }

        match load_manifest(&dep_root) {
            Ok(m) => {
                out.push_str(&format!(
                    "{prefix}{connector}{name} {} (path: {path_str})\n",
                    m.package.version
                ));
                // Push onto the DFS stack; pop after children so shared deps
                // (diamond DAG) are not incorrectly flagged as cycles.
                visited.insert(canonical.clone());
                let mut sub_deps: Vec<(&String, &Dependency)> =
                    m.dependencies.iter().collect();
                sub_deps.sort_by_key(|(k, _)| k.as_str());
                let sub_count = sub_deps.len();
                for (i, (sub_name, sub_dep)) in sub_deps.iter().enumerate() {
                    print_dep(
                        out,
                        sub_name,
                        sub_dep,
                        &dep_root,
                        &child_prefix,
                        i == sub_count - 1,
                        visited,
                    );
                }
                visited.remove(&canonical);
            }
            Err(e) => {
                out.push_str(&format!(
                    "{prefix}{connector}{name} (path: {path_str}) [error: {e}]\n"
                ));
            }
        }
    } else if let Some(url) = &dep.git {
        let rev = dep.rev.as_deref().unwrap_or("?");
        out.push_str(&format!(
            "{prefix}{connector}{name} (git: {url}, rev: {rev}) [not synced]\n"
        ));
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn make_manifest(dir: &Path, name: &str) {
        fs::write(
            dir.join("Tyra.toml"),
            format!(
                "[package]\nname    = \"{name}\"\nversion = \"0.1.0\"\nedition = \"2026\"\n"
            ),
        )
        .unwrap();
    }

    // --- run_init ---

    #[test]
    fn init_creates_manifest() {
        let dir = tempfile::tempdir().unwrap();
        run_init(dir.path(), Some("myapp")).unwrap();
        let content = fs::read_to_string(dir.path().join("Tyra.toml")).unwrap();
        assert!(content.contains("name    = \"myapp\""));
        assert!(content.contains("edition = \"2026\""));
    }

    #[test]
    fn init_infers_name_from_dirname() {
        let dir = tempfile::tempdir().unwrap();
        let sub = dir.path().join("my_project");
        fs::create_dir(&sub).unwrap();
        run_init(&sub, None).unwrap();
        let content = fs::read_to_string(sub.join("Tyra.toml")).unwrap();
        assert!(content.contains("name    = \"my_project\""));
    }

    #[test]
    fn init_fails_if_manifest_exists() {
        let dir = tempfile::tempdir().unwrap();
        make_manifest(dir.path(), "myapp");
        let result = run_init(dir.path(), Some("myapp"));
        assert!(matches!(result, Err(PkgError::AlreadyExists(_))));
    }

    #[test]
    fn init_rejects_uppercase_name() {
        let dir = tempfile::tempdir().unwrap();
        let result = run_init(dir.path(), Some("MyApp"));
        assert!(matches!(result, Err(PkgError::InvalidName(_))));
    }

    #[test]
    fn init_rejects_reserved_word() {
        let dir = tempfile::tempdir().unwrap();
        let result = run_init(dir.path(), Some("match"));
        assert!(matches!(result, Err(PkgError::InvalidName(_))));
    }

    // --- run_add ---

    #[test]
    fn add_path_dep_creates_dependencies_section() {
        let dir = tempfile::tempdir().unwrap();
        make_manifest(dir.path(), "myapp");
        run_add(dir.path(), "mylib", DepSource::Path("../mylib".into())).unwrap();
        let content = fs::read_to_string(dir.path().join("Tyra.toml")).unwrap();
        assert!(content.contains("[dependencies]"));
        assert!(content.contains("mylib = { path = \"../mylib\" }"));
    }

    #[test]
    fn add_path_dep_appends_to_existing_section() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(
            dir.path().join("Tyra.toml"),
            "[package]\nname    = \"myapp\"\nversion = \"0.1.0\"\nedition = \"2026\"\n\
             \n[dependencies]\nmylib = { path = \"../mylib\" }\n",
        )
        .unwrap();
        run_add(dir.path(), "utils", DepSource::Path("../utils".into())).unwrap();
        let content = fs::read_to_string(dir.path().join("Tyra.toml")).unwrap();
        assert!(content.contains("mylib = { path = \"../mylib\" }"));
        assert!(content.contains("utils = { path = \"../utils\" }"));
        let mylib_pos = content.find("mylib").unwrap();
        let utils_pos = content.find("utils").unwrap();
        assert!(utils_pos > mylib_pos, "utils must appear after mylib");
    }

    #[test]
    fn add_git_dep() {
        let dir = tempfile::tempdir().unwrap();
        make_manifest(dir.path(), "myapp");
        run_add(
            dir.path(),
            "utils",
            DepSource::Git {
                url: "https://github.com/example/utils.git".into(),
                rev: "abc1234".into(),
            },
        )
        .unwrap();
        let content = fs::read_to_string(dir.path().join("Tyra.toml")).unwrap();
        assert!(content.contains("git = \"https://github.com/example/utils.git\""));
        assert!(content.contains("rev = \"abc1234\""));
    }

    #[test]
    fn add_duplicate_dep_is_error() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(
            dir.path().join("Tyra.toml"),
            "[package]\nname    = \"myapp\"\nversion = \"0.1.0\"\nedition = \"2026\"\n\
             \n[dependencies]\nmylib = { path = \"../mylib\" }\n",
        )
        .unwrap();
        let result = run_add(dir.path(), "mylib", DepSource::Path("../mylib".into()));
        assert!(matches!(result, Err(PkgError::DuplicateDep(_))));
    }

    #[test]
    fn add_rejects_invalid_dep_name() {
        let dir = tempfile::tempdir().unwrap();
        make_manifest(dir.path(), "myapp");
        let result = run_add(dir.path(), "MyLib", DepSource::Path("../mylib".into()));
        assert!(matches!(result, Err(PkgError::InvalidName(_))));
    }

    #[test]
    fn add_path_dep_name_mismatch_is_error() {
        let root = tempfile::tempdir().unwrap();
        let lib_dir = tempfile::tempdir().unwrap();
        // Dep key "utils" but the dep's package.name is "mylib"
        make_manifest(lib_dir.path(), "mylib");
        make_manifest(root.path(), "myapp");
        let result = run_add(
            root.path(),
            "utils",
            DepSource::Path(lib_dir.path().to_str().unwrap().to_string()),
        );
        assert!(
            matches!(result, Err(PkgError::NameMismatch { .. })),
            "expected NameMismatch, got: {result:?}"
        );
    }

    #[test]
    fn add_path_dep_name_match_succeeds() {
        let root = tempfile::tempdir().unwrap();
        let lib_dir = tempfile::tempdir().unwrap();
        // Dep key matches package.name — should succeed.
        make_manifest(lib_dir.path(), "mylib");
        make_manifest(root.path(), "myapp");
        run_add(
            root.path(),
            "mylib",
            DepSource::Path(lib_dir.path().to_str().unwrap().to_string()),
        )
        .unwrap();
        let content = std::fs::read_to_string(root.path().join("Tyra.toml")).unwrap();
        assert!(content.contains("mylib = { path ="));
    }

    // --- run_tree ---

    #[test]
    fn tree_no_deps() {
        let dir = tempfile::tempdir().unwrap();
        make_manifest(dir.path(), "myapp");
        let tree = run_tree(dir.path()).unwrap();
        assert_eq!(tree, "myapp 0.1.0\n");
    }

    #[test]
    fn tree_with_path_dep() {
        let root = tempfile::tempdir().unwrap();
        let lib_dir = tempfile::tempdir().unwrap();
        make_manifest(lib_dir.path(), "mylib");

        fs::write(
            root.path().join("Tyra.toml"),
            format!(
                "[package]\nname    = \"myapp\"\nversion = \"0.1.0\"\nedition = \"2026\"\n\
                 \n[dependencies]\nmylib = {{ path = \"{}\" }}\n",
                lib_dir.path().display()
            ),
        )
        .unwrap();

        let tree = run_tree(root.path()).unwrap();
        assert!(tree.starts_with("myapp 0.1.0\n"));
        assert!(tree.contains("mylib 0.1.0"));
        assert!(tree.contains("└── "));
    }

    #[test]
    fn tree_git_dep_shown_as_not_synced() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(
            dir.path().join("Tyra.toml"),
            "[package]\nname    = \"myapp\"\nversion = \"0.1.0\"\nedition = \"2026\"\n\
             \n[dependencies]\nutils = { git = \"https://github.com/example/utils.git\", \
             rev = \"abc1234\" }\n",
        )
        .unwrap();
        let tree = run_tree(dir.path()).unwrap();
        assert!(tree.contains("[not synced]"));
        assert!(tree.contains("utils"));
    }

    #[test]
    fn tree_cycle_is_detected() {
        let dir_a = tempfile::tempdir().unwrap();
        let dir_b = tempfile::tempdir().unwrap();

        fs::write(
            dir_a.path().join("Tyra.toml"),
            format!(
                "[package]\nname    = \"pkg_a\"\nversion = \"0.1.0\"\nedition = \"2026\"\n\
                 \n[dependencies]\npkg_b = {{ path = \"{}\" }}\n",
                dir_b.path().display()
            ),
        )
        .unwrap();
        fs::write(
            dir_b.path().join("Tyra.toml"),
            format!(
                "[package]\nname    = \"pkg_b\"\nversion = \"0.1.0\"\nedition = \"2026\"\n\
                 \n[dependencies]\npkg_a = {{ path = \"{}\" }}\n",
                dir_a.path().display()
            ),
        )
        .unwrap();

        let tree = run_tree(dir_a.path()).unwrap();
        assert!(tree.contains("[cycle]"));
    }

    #[test]
    fn tree_shared_dep_diamond_not_flagged_as_cycle() {
        // app -> a -> common
        // app -> b -> common
        // `common` is shared (diamond DAG), not a cycle.
        let dir_app = tempfile::tempdir().unwrap();
        let dir_a = tempfile::tempdir().unwrap();
        let dir_b = tempfile::tempdir().unwrap();
        let dir_common = tempfile::tempdir().unwrap();

        make_manifest(dir_common.path(), "common");
        fs::write(
            dir_a.path().join("Tyra.toml"),
            format!(
                "[package]\nname    = \"pkg_a\"\nversion = \"0.1.0\"\nedition = \"2026\"\n\
                 \n[dependencies]\ncommon = {{ path = \"{}\" }}\n",
                dir_common.path().display()
            ),
        )
        .unwrap();
        fs::write(
            dir_b.path().join("Tyra.toml"),
            format!(
                "[package]\nname    = \"pkg_b\"\nversion = \"0.1.0\"\nedition = \"2026\"\n\
                 \n[dependencies]\ncommon = {{ path = \"{}\" }}\n",
                dir_common.path().display()
            ),
        )
        .unwrap();
        fs::write(
            dir_app.path().join("Tyra.toml"),
            format!(
                "[package]\nname    = \"app\"\nversion = \"0.1.0\"\nedition = \"2026\"\n\
                 \n[dependencies]\npkg_a = {{ path = \"{}\" }}\npkg_b = {{ path = \"{}\" }}\n",
                dir_a.path().display(),
                dir_b.path().display()
            ),
        )
        .unwrap();

        let tree = run_tree(dir_app.path()).unwrap();
        assert!(!tree.contains("[cycle]"), "diamond DAG must not be flagged as cycle:\n{tree}");
        assert_eq!(tree.matches("common").count(), 2, "common should appear twice:\n{tree}");
    }

    // --- validate_dep_root (cache-hit path regression tests) ---

    fn make_src_file(dir: &Path, name: &str, content: &str) {
        let src = dir.join("src");
        fs::create_dir_all(&src).unwrap();
        fs::write(src.join(format!("{name}.tyra")), content).unwrap();
    }

    #[test]
    fn cached_dep_valid_passes() {
        let dir = tempfile::tempdir().unwrap();
        make_manifest(dir.path(), "mylib");
        make_src_file(dir.path(), "mylib", "export fn greet(name: String) -> String\n  name\nend\n");
        validate_dep_root("mylib", dir.path()).unwrap();
    }

    #[test]
    fn cached_dep_name_mismatch_is_error() {
        let dir = tempfile::tempdir().unwrap();
        // package.name = "mylib" but dep key = "utils"
        make_manifest(dir.path(), "mylib");
        let result = validate_dep_root("utils", dir.path());
        assert!(
            matches!(result, Err(PkgError::NameMismatch { .. })),
            "expected NameMismatch, got: {result:?}"
        );
    }

    #[test]
    fn cached_dep_bin_package_is_error() {
        let dir = tempfile::tempdir().unwrap();
        make_manifest(dir.path(), "myapp");
        // Root source contains `fn main` — bin package.
        make_src_file(dir.path(), "myapp", "fn main() -> Unit\n  print(\"hi\")\nend\n");
        let result = validate_dep_root("myapp", dir.path());
        assert!(
            matches!(result, Err(PkgError::BinDepNotImportable(_))),
            "expected BinDepNotImportable, got: {result:?}"
        );
    }

    #[test]
    fn cached_dep_no_src_file_is_error() {
        // ADR 0009: root module src/<name>.tyra must exist; absence is an error.
        let dir = tempfile::tempdir().unwrap();
        make_manifest(dir.path(), "mylib");
        let result = validate_dep_root("mylib", dir.path());
        assert!(
            matches!(result, Err(PkgError::MissingRootModule(_))),
            "expected MissingRootModule, got: {result:?}"
        );
    }

    // --- insert_dependency_line (unit) ---

    #[test]
    fn insert_creates_section_when_absent() {
        let content =
            "[package]\nname    = \"x\"\nversion = \"0.1.0\"\nedition = \"2026\"\n";
        let result = insert_dependency_line(content, "foo = { path = \"../foo\" }");
        assert!(result.contains("[dependencies]"));
        assert!(result.contains("foo = { path = \"../foo\" }"));
    }

    #[test]
    fn insert_appends_within_existing_section() {
        let content =
            "[package]\nname    = \"x\"\nversion = \"0.1.0\"\nedition = \"2026\"\n\
             \n[dependencies]\nalpha = { path = \"../alpha\" }\n";
        let result = insert_dependency_line(content, "beta = { path = \"../beta\" }");
        let alpha_pos = result.find("alpha").unwrap();
        let beta_pos = result.find("beta").unwrap();
        assert!(beta_pos > alpha_pos, "beta must come after alpha");
    }

    #[test]
    fn insert_preserves_trailing_newline() {
        let content = "[package]\nname    = \"x\"\nversion = \"0.1.0\"\nedition = \"2026\"\n";
        let result = insert_dependency_line(content, "foo = { path = \"../foo\" }");
        assert!(result.ends_with('\n'));
    }

    // --- run_remove ---

    #[test]
    fn remove_dep_removes_the_line() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(
            dir.path().join("Tyra.toml"),
            "[package]\nname    = \"myapp\"\nversion = \"0.1.0\"\nedition = \"2026\"\n\
             \n[dependencies]\nmylib = { path = \"../mylib\" }\nutils = { path = \"../utils\" }\n",
        )
        .unwrap();
        run_remove(dir.path(), "mylib").unwrap();
        let content = fs::read_to_string(dir.path().join("Tyra.toml")).unwrap();
        assert!(!content.contains("mylib"), "mylib must be removed");
        assert!(content.contains("utils = { path"), "utils must remain");
    }

    #[test]
    fn remove_nonexistent_dep_is_error() {
        let dir = tempfile::tempdir().unwrap();
        make_manifest(dir.path(), "myapp");
        let result = run_remove(dir.path(), "nonexistent");
        assert!(matches!(result, Err(PkgError::DepNotFound(_))));
    }

    // --- remove_dependency_line (unit) ---

    #[test]
    fn remove_line_leaves_section_header() {
        let content =
            "[package]\nname    = \"x\"\nversion = \"0.1.0\"\nedition = \"2026\"\n\
             \n[dependencies]\nalpha = { path = \"../alpha\" }\n";
        let result = remove_dependency_line(content, "alpha");
        assert!(result.contains("[dependencies]"), "header must remain");
        assert!(!result.contains("alpha = "));
    }

    #[test]
    fn remove_line_preserves_others() {
        let content =
            "[package]\nname    = \"x\"\nversion = \"0.1.0\"\nedition = \"2026\"\n\
             \n[dependencies]\nalpha = { path = \"../alpha\" }\nbeta = { path = \"../beta\" }\n";
        let result = remove_dependency_line(content, "alpha");
        assert!(!result.contains("alpha = "));
        assert!(result.contains("beta = { path = \"../beta\" }"));
    }

    // --- run_sync_check ---

    #[test]
    fn sync_check_path_dep_valid_passes() {
        let root = tempfile::tempdir().unwrap();
        let lib_dir = tempfile::tempdir().unwrap();
        make_manifest(lib_dir.path(), "mylib");
        // Add root module
        let src = lib_dir.path().join("src");
        fs::create_dir_all(&src).unwrap();
        fs::write(src.join("mylib.tyra"), "export fn greet() -> String\n  \"hi\"\nend\n").unwrap();
        fs::write(
            root.path().join("Tyra.toml"),
            format!(
                "[package]\nname    = \"myapp\"\nversion = \"0.1.0\"\nedition = \"2026\"\n\
                 \n[dependencies]\nmylib = {{ path = \"{}\" }}\n",
                lib_dir.path().display()
            ),
        )
        .unwrap();
        let issues = run_sync_check(root.path()).unwrap();
        assert!(issues.is_empty(), "expected no issues, got: {issues:?}");
    }

    #[test]
    fn sync_check_unsynced_git_dep_is_issue() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(
            dir.path().join("Tyra.toml"),
            "[package]\nname    = \"myapp\"\nversion = \"0.1.0\"\nedition = \"2026\"\n\
             \n[dependencies]\nutils = { git = \"https://github.com/example/utils.git\", \
             rev = \"abc1234\" }\n",
        )
        .unwrap();
        let issues = run_sync_check(dir.path()).unwrap();
        assert!(!issues.is_empty(), "unsynced git dep must be flagged");
        assert!(issues[0].contains("not synced"));
    }

    // --- run_tree_json ---

    #[test]
    fn tree_json_no_deps_is_valid_root_object() {
        let dir = tempfile::tempdir().unwrap();
        make_manifest(dir.path(), "myapp");
        let json = run_tree_json(dir.path()).unwrap();
        assert!(json.contains("\"name\":\"myapp\""), "missing name field: {json}");
        assert!(json.contains("\"version\":\"0.1.0\""), "missing version field: {json}");
        assert!(json.contains("\"deps\":[]"), "empty deps array missing: {json}");
    }

    #[test]
    fn tree_json_path_dep_nested_in_parent_deps() {
        // Regression: the old tree_to_json() flattened all nodes to root.deps.
        // dep_node_json() must produce: root.deps[0].deps[0] == child, not root.deps[1].
        let root = tempfile::tempdir().unwrap();
        let lib_dir = tempfile::tempdir().unwrap();
        let child_dir = tempfile::tempdir().unwrap();

        // child has no deps
        make_manifest(child_dir.path(), "child");
        // lib depends on child
        fs::write(
            lib_dir.path().join("Tyra.toml"),
            format!(
                "[package]\nname    = \"mylib\"\nversion = \"0.1.0\"\nedition = \"2026\"\n\
                 \n[dependencies]\nchild = {{ path = \"{}\" }}\n",
                child_dir.path().display()
            ),
        )
        .unwrap();
        // root depends on lib
        fs::write(
            root.path().join("Tyra.toml"),
            format!(
                "[package]\nname    = \"myapp\"\nversion = \"0.1.0\"\nedition = \"2026\"\n\
                 \n[dependencies]\nmylib = {{ path = \"{}\" }}\n",
                lib_dir.path().display()
            ),
        )
        .unwrap();

        let json = run_tree_json(root.path()).unwrap();

        // child must appear inside mylib's deps, not at the root deps level.
        // A flat (broken) output would contain "child" at root.deps; the correct
        // output nests it inside the mylib node's own deps array.
        // Verify by checking the structure: root.deps has one element (mylib),
        // which itself has a non-empty deps array containing child.
        assert!(json.contains("\"key\":\"mylib\""), "mylib missing: {json}");
        assert!(json.contains("\"key\":\"child\""), "child missing: {json}");

        // In the correct nested output the child node comes *after* the mylib
        // node's opening brace, i.e., inside its deps array.
        let mylib_pos = json.find("\"key\":\"mylib\"").unwrap();
        let child_pos = json.find("\"key\":\"child\"").unwrap();
        assert!(
            child_pos > mylib_pos,
            "child must be nested inside mylib, not at root level: {json}"
        );

        // Root deps array must contain exactly one direct element (mylib).
        // Count top-level "key" occurrences inside root.deps [...] by checking
        // that only one "\"key\"" appears at the root of the object (before the
        // first nested "{" at depth > 1). We do this structurally: parse the
        // root deps array extent and count only the first-level keys.
        // Simpler proxy: root has one direct dep key "mylib"; child is not a
        // sibling of mylib in the JSON text at the first deps level.
        // We verify this by confirming the JSON starts with the root object and
        // root.deps is a single-element array.
        let after_root_deps = json.find("\"deps\":[").unwrap();
        let root_deps_content = &json[after_root_deps..];
        // The root deps array ends at the first ']' that closes it (depth 1).
        // Count that only one "\"key\":" token lives at depth 1 within root.deps.
        let mut depth = 0usize;
        let mut top_level_keys = 0usize;
        let chars: Vec<char> = root_deps_content.chars().collect();
        let mut i = 0;
        while i < chars.len() {
            match chars[i] {
                '[' | '{' => depth += 1,
                ']' | '}' => {
                    depth = depth.saturating_sub(1);
                    if depth == 0 { break; }
                }
                '"' if depth == 2 => {
                    // Check for "key": pattern at depth 2 (inside root.deps[x])
                    let rest: String = chars[i..].iter().collect();
                    if rest.starts_with("\"key\":") {
                        top_level_keys += 1;
                    }
                }
                _ => {}
            }
            i += 1;
        }
        assert_eq!(top_level_keys, 1, "root.deps must have exactly 1 direct element: {json}");
    }

    #[test]
    fn tree_json_cycle_emits_cycle_true() {
        let dir_a = tempfile::tempdir().unwrap();
        let dir_b = tempfile::tempdir().unwrap();
        fs::write(
            dir_a.path().join("Tyra.toml"),
            format!(
                "[package]\nname    = \"pkg_a\"\nversion = \"0.1.0\"\nedition = \"2026\"\n\
                 \n[dependencies]\npkg_b = {{ path = \"{}\" }}\n",
                dir_b.path().display()
            ),
        )
        .unwrap();
        fs::write(
            dir_b.path().join("Tyra.toml"),
            format!(
                "[package]\nname    = \"pkg_b\"\nversion = \"0.1.0\"\nedition = \"2026\"\n\
                 \n[dependencies]\npkg_a = {{ path = \"{}\" }}\n",
                dir_a.path().display()
            ),
        )
        .unwrap();
        let json = run_tree_json(dir_a.path()).unwrap();
        assert!(json.contains("\"cycle\":true"), "cycle node missing: {json}");
    }

    #[test]
    fn tree_json_git_dep_emits_synced_false() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(
            dir.path().join("Tyra.toml"),
            "[package]\nname    = \"myapp\"\nversion = \"0.1.0\"\nedition = \"2026\"\n\
             \n[dependencies]\nutils = { git = \"https://github.com/example/utils.git\", \
             rev = \"abc1234\" }\n",
        )
        .unwrap();
        let json = run_tree_json(dir.path()).unwrap();
        assert!(json.contains("\"synced\":false"), "git dep must report synced:false: {json}");
        assert!(json.contains("\"key\":\"utils\""), "utils key missing: {json}");
        assert!(json.contains("\"rev\":\"abc1234\""), "rev missing: {json}");
    }

    #[test]
    fn tree_json_diamond_not_flagged_as_cycle() {
        // app -> a -> common
        // app -> b -> common
        // common appears twice (diamond), neither node should have "cycle":true.
        let dir_app = tempfile::tempdir().unwrap();
        let dir_a = tempfile::tempdir().unwrap();
        let dir_b = tempfile::tempdir().unwrap();
        let dir_common = tempfile::tempdir().unwrap();
        make_manifest(dir_common.path(), "common");
        fs::write(
            dir_a.path().join("Tyra.toml"),
            format!(
                "[package]\nname    = \"pkg_a\"\nversion = \"0.1.0\"\nedition = \"2026\"\n\
                 \n[dependencies]\ncommon = {{ path = \"{}\" }}\n",
                dir_common.path().display()
            ),
        )
        .unwrap();
        fs::write(
            dir_b.path().join("Tyra.toml"),
            format!(
                "[package]\nname    = \"pkg_b\"\nversion = \"0.1.0\"\nedition = \"2026\"\n\
                 \n[dependencies]\ncommon = {{ path = \"{}\" }}\n",
                dir_common.path().display()
            ),
        )
        .unwrap();
        fs::write(
            dir_app.path().join("Tyra.toml"),
            format!(
                "[package]\nname    = \"app\"\nversion = \"0.1.0\"\nedition = \"2026\"\n\
                 \n[dependencies]\npkg_a = {{ path = \"{}\" }}\npkg_b = {{ path = \"{}\" }}\n",
                dir_a.path().display(),
                dir_b.path().display()
            ),
        )
        .unwrap();
        let json = run_tree_json(dir_app.path()).unwrap();
        assert!(!json.contains("\"cycle\":true"), "diamond must not be flagged as cycle: {json}");
        assert_eq!(json.matches("\"key\":\"common\"").count(), 2, "common must appear twice: {json}");
    }
}
