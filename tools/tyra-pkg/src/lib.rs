//! tyra-pkg: dependency management commands for the Tyra language.
//!
//! Public API:
//! - `run_init(dest, name)` — create Tyra.toml in an existing directory
//! - `run_add(project_root, dep_name, source)` — append a dependency entry
//! - `run_update(project_root, dep_name, source)` — update an existing entry in-place
//! - `run_remove(project_root, dep_name)` — delete a dependency entry
//! - `run_show(project_root, dep_name)` — human-readable dependency details
//! - `run_show_json(project_root, dep_name)` — JSON dependency details
//! - `run_tree(project_root)` — render the dependency tree as a string
//! - `run_tree_json(project_root)` — dependency tree as JSON
//! - `run_sync(project_root)` — clone git deps into `~/.tyra/cache/git/`
//! - `run_sync_check(project_root)` — validate deps without mutating
//! - `run_clean()` — remove the entire `~/.tyra/cache/` directory
//! - `tyra_cache_root()` — path to the Tyra cache root (`~/.tyra/cache/`)
//! - `cache_dir_for(dep_name, url, rev)` — canonical cache path for a git dep

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::process::Command;
use tyra_manifest::{
    Dependency, LockedPackage, LockfileError, build_and_write_lockfile, find_project_root,
    load_lockfile, load_manifest,
};

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

// Spec §5.2 reserved words — dep names must not collide with these.
const RESERVED_WORDS: &[&str] = &[
    "fn", "data", "value", "type", "trait", "impl", "let", "mut", "if", "else", "match", "when",
    "for", "in", "while", "return", "defer", "async", "await", "spawn", "import", "export", "and",
    "or", "not", "true", "false", "end",
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
    SyncFailed {
        dep: String,
        message: String,
    },
    /// Dependency root is a bin package (ADR 0009 E_DEP_NOT_IMPORTABLE).
    BinDepNotImportable(String),
    /// Dependency key does not match the package name declared in `Tyra.toml`.
    NameMismatch {
        key: String,
        package_name: String,
    },
    /// Root module `src/<name>.tyra` is absent (ADR 0009 requires it).
    MissingRootModule(String),
    /// Same package required by two paths with incompatible revisions (E0218).
    DepConflict {
        name: String,
        rev_existing: String,
        rev_new: String,
    },
    /// Two distinct sources both claim the same import name (E0220).
    /// Import identifier == package name (ADR 0009/0010), so this is an
    /// unresolvable namespace collision, not a rev mismatch.
    DepNameCollision {
        name: String,
        source_existing: String,
        source_new: String,
    },
    /// `Tyra.lock` I/O or parse error.
    Lockfile(LockfileError),
    /// `--locked` was requested but `Tyra.lock` does not exist.
    LockfileNotFound,
    /// `--locked` was requested but `Tyra.toml` has a direct dep not in the lockfile.
    LockfileOutOfSync(String),
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
            PkgError::DepConflict {
                name,
                rev_existing,
                rev_new,
            } => write!(
                f,
                "error[E0218]: dependency conflict for `{name}`: \
                 required at rev `{rev_existing}` and `{rev_new}` — \
                 update your manifests to use a single revision"
            ),
            PkgError::DepNameCollision {
                name,
                source_existing,
                source_new,
            } => write!(
                f,
                "error[E0220]: import name `{name}` is claimed by two different sources: \
                 `{source_existing}` and `{source_new}` — \
                 a single import name must resolve to exactly one package"
            ),
            PkgError::Lockfile(e) => write!(f, "Tyra.lock error: {e}"),
            PkgError::LockfileNotFound => write!(
                f,
                "Tyra.lock not found; run `tyra mod sync` first (or remove --locked)"
            ),
            PkgError::LockfileOutOfSync(dep) => write!(
                f,
                "error[E0219]: `{dep}` is in Tyra.toml but missing from Tyra.lock; \
                 run `tyra mod sync` to regenerate the lockfile"
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

impl From<LockfileError> for PkgError {
    fn from(e: LockfileError) -> Self {
        PkgError::Lockfile(e)
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
    let content =
        format!("[package]\nname    = \"{pkg_name}\"\nversion = \"0.1.0\"\nedition = \"2026\"\n");
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
        if let Ok(dep_manifest) = load_manifest(&dep_root)
            && dep_manifest.package.name != dep_name
        {
            return Err(PkgError::NameMismatch {
                key: dep_name.to_string(),
                package_name: dep_manifest.package.name.clone(),
            });
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
    out.push_str(&format!(
        "{} {}\n",
        manifest.package.name, manifest.package.version
    ));

    let mut visited = HashSet::new();
    let canonical = project_root
        .canonicalize()
        .unwrap_or_else(|_| project_root.to_path_buf());
    visited.insert(canonical);

    let mut deps: Vec<(&String, &Dependency)> = manifest.dependencies.iter().collect();
    deps.sort_by_key(|(k, _)| k.as_str());

    let count = deps.len();
    for (i, (name, dep)) in deps.iter().enumerate() {
        print_dep(
            &mut out,
            name,
            dep,
            project_root,
            "",
            i == count - 1,
            &mut visited,
        );
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
    let canonical = project_root
        .canonicalize()
        .unwrap_or_else(|_| project_root.to_path_buf());
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
pub fn run_add_from(start: &Path, dep_name: &str, source: DepSource) -> Result<(), PkgError> {
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

/// `tyra mod update <dep_name> --path <path>` / `--git <url> --rev <rev>`
///
/// Replaces an existing `[dependencies]` entry in-place. The entry must
/// already exist (`DepNotFound` otherwise). For path deps the key/name
/// invariant is validated, same as `run_add`.
pub fn run_update(project_root: &Path, dep_name: &str, source: DepSource) -> Result<(), PkgError> {
    validate_name(dep_name)?;
    let manifest = load_manifest(project_root)?;
    if !manifest.dependencies.contains_key(dep_name) {
        return Err(PkgError::DepNotFound(dep_name.to_string()));
    }
    if let DepSource::Path(rel) = &source {
        let dep_root = project_root.join(rel);
        if let Ok(dep_manifest) = load_manifest(&dep_root)
            && dep_manifest.package.name != dep_name
        {
            return Err(PkgError::NameMismatch {
                key: dep_name.to_string(),
                package_name: dep_manifest.package.name.clone(),
            });
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
    let updated = replace_dependency_line(&content, dep_name, &new_line);
    std::fs::write(&manifest_path, updated)?;
    Ok(())
}

/// Locate the project root walking up from `start`, then call `run_update`.
pub fn run_update_from(start: &Path, dep_name: &str, source: DepSource) -> Result<(), PkgError> {
    let root = find_project_root(start).ok_or(PkgError::NoProject)?;
    run_update(&root, dep_name, source)
}

/// `tyra mod sync`
///
/// Clones all git dependencies declared in `project_root/Tyra.toml` into
/// `~/.tyra/cache/git/<dep_name>/<rev>/`.  Path dependencies are skipped.
/// Resolve, fetch, and lock all direct + transitive dependencies.
///
/// After a successful run `Tyra.lock` is written (or updated) in `project_root`.
pub fn run_sync(project_root: &Path) -> Result<SyncReport, PkgError> {
    let mut report = SyncReport::default();
    let mut resolved: HashMap<String, ResolvedEntry> = HashMap::new();
    let original_root = project_root
        .canonicalize()
        .unwrap_or_else(|_| project_root.to_path_buf());

    resolve_transitive_impl(
        &original_root,
        &original_root,
        &mut resolved,
        &mut report,
        0,
        &original_root,
    )?;

    // Build and write Tyra.lock.
    let packages: Vec<LockedPackage> = resolved
        .values()
        .map(|e| LockedPackage {
            name: e.name.clone(),
            source: e.source.clone(),
            rev: e.rev.clone(),
            branch: e.branch.clone(),
            pkg_version: e.pkg_version.clone(),
        })
        .collect();
    build_and_write_lockfile(project_root, packages).map_err(PkgError::Io)?;

    Ok(report)
}

/// Locate the project root walking up from `start`, then call `run_sync`.
pub fn run_sync_from(start: &Path) -> Result<SyncReport, PkgError> {
    let root = find_project_root(start).ok_or(PkgError::NoProject)?;
    run_sync(&root)
}

/// CI mode: read an existing `Tyra.lock` and verify the cache is populated.
///
/// Does **not** fetch anything new or update the lockfile.  Fails if:
/// - `Tyra.lock` is absent,
/// - a direct dependency in `Tyra.toml` is missing from the lock, or
/// - a locked git dep is not in the cache.
pub fn run_sync_locked(project_root: &Path) -> Result<SyncReport, PkgError> {
    let lf = load_lockfile(project_root)?.ok_or(PkgError::LockfileNotFound)?;
    let original_root = project_root
        .canonicalize()
        .unwrap_or_else(|_| project_root.to_path_buf());

    // Verify every direct dep in Tyra.toml is present in the lockfile.
    // Match by canonical source (not by alias) so that alias renames are caught.
    let manifest = load_manifest(&original_root)?;
    for (dep_name, dep) in &manifest.dependencies {
        let expected_source = if let Some(rel) = &dep.path {
            let abs = normalize_path(&original_root.join(rel));
            format!("path+{}", path_relative_to(&abs, &original_root).display())
        } else if let Some(url) = &dep.git {
            format!("git+{url}")
        } else {
            continue;
        };

        let pkg = lf
            .packages
            .iter()
            .find(|p| p.source == expected_source)
            .ok_or_else(|| {
                PkgError::LockfileOutOfSync(format!(
                    "{dep_name}: source `{expected_source}` not in Tyra.lock (run `tyra mod sync`)"
                ))
            })?;

        // For pinned-rev git deps, verify the rev hasn't changed.
        if let Some(manifest_rev) = &dep.rev
            && pkg.rev.as_deref() != Some(manifest_rev.as_str())
        {
            return Err(PkgError::LockfileOutOfSync(format!(
                "{dep_name}: rev changed from `{}` to `{manifest_rev}`",
                pkg.rev.as_deref().unwrap_or("?")
            )));
        }
        // For branch deps, verify the branch name hasn't changed.
        if let Some(manifest_branch) = &dep.branch
            && pkg.branch.as_deref() != Some(manifest_branch.as_str())
        {
            return Err(PkgError::LockfileOutOfSync(format!(
                "{dep_name}: branch changed from `{}` to `{manifest_branch}` (run `tyra mod sync`)",
                pkg.branch.as_deref().unwrap_or("?")
            )));
        }
        // Constraint type must also match: changing branch→rev or rev→branch requires
        // a fresh sync even if the SHA happens to be the same right now.
        if dep.rev.is_some() && dep.branch.is_none() && pkg.branch.is_some() {
            return Err(PkgError::LockfileOutOfSync(format!(
                "{dep_name}: constraint changed from branch `{}` to rev `{}` (run `tyra mod sync`)",
                pkg.branch.as_deref().unwrap_or("?"),
                dep.rev.as_deref().unwrap_or("?")
            )));
        }
        if dep.branch.is_some() && dep.rev.is_none() && pkg.branch.is_none() {
            return Err(PkgError::LockfileOutOfSync(format!(
                "{dep_name}: constraint changed from rev to branch `{}` (run `tyra mod sync`)",
                dep.branch.as_deref().unwrap_or("?")
            )));
        }
        // The dep key (alias) must still equal the lockfile's recorded name.
        // run_sync / run_sync_check enforce dep_key == package_name, so a rename
        // that hasn't been re-synced would pass source matching but break imports.
        if dep_name != &pkg.name {
            return Err(PkgError::LockfileOutOfSync(format!(
                "{dep_name}: dep key was `{}` when lockfile was last generated \
                 (run `tyra mod sync`)",
                pkg.name
            )));
        }
        // For path deps, also validate the dep structure with the current key — the
        // same check run_sync and run_sync_check perform.
        if let Some(rel) = &dep.path {
            let dep_root = normalize_path(&original_root.join(rel));
            if let Err(e) = validate_dep_root(dep_name, &dep_root) {
                return Err(PkgError::SyncFailed {
                    dep: dep_name.clone(),
                    message: e.to_string(),
                });
            }
        }
    }

    // Verify transitive path deps haven't changed.
    //
    // Re-walk the local path-dep tree (no network) and compare the live set of
    // canonical path sources against what the lockfile records.  A mismatch means
    // a nested Tyra.toml was edited without re-running `tyra mod sync`.
    let mut live_path_sources: HashSet<String> = HashSet::new();
    collect_path_dep_sources(&original_root, &original_root, &mut live_path_sources, 0)?;

    let locked_path_sources: HashSet<String> = lf
        .packages
        .iter()
        .filter(|p| p.source.starts_with("path+"))
        .map(|p| p.source.clone())
        .collect();

    for src in &live_path_sources {
        if !locked_path_sources.contains(src.as_str()) {
            return Err(PkgError::LockfileOutOfSync(format!(
                "new transitive path dep `{src}` not in Tyra.lock (run `tyra mod sync`)"
            )));
        }
    }
    for src in &locked_path_sources {
        if !live_path_sources.contains(src.as_str()) {
            return Err(PkgError::LockfileOutOfSync(format!(
                "path dep `{src}` removed from project but still in Tyra.lock (run `tyra mod sync`)"
            )));
        }
    }

    let mut report = SyncReport::default();
    for pkg in &lf.packages {
        if let Some(rev) = &pkg.rev {
            // git dep — extract url from "git+<url>"
            let url = pkg.source.strip_prefix("git+").unwrap_or(&pkg.source);
            let cache_dir = cache_dir_for(&pkg.name, url, rev);
            if cache_dir.join("Tyra.toml").is_file() {
                report.cached.push(pkg.name.clone());
            } else {
                return Err(PkgError::SyncFailed {
                    dep: pkg.name.clone(),
                    message: format!(
                        "not in cache at expected path `{}`; run `tyra mod sync` to populate",
                        cache_dir.display()
                    ),
                });
            }
        } else {
            // path dep — source is relative to original_root, validate root structure
            let dep_path = pkg.source.strip_prefix("path+").unwrap_or(&pkg.source);
            let dep_root = original_root.join(dep_path);
            if let Err(e) = validate_dep_root(&pkg.name, &dep_root) {
                return Err(PkgError::SyncFailed {
                    dep: pkg.name.clone(),
                    message: e.to_string(),
                });
            }
            report.skipped.push(pkg.name.clone());
        }
    }
    Ok(report)
}

/// Locate the project root walking up from `start`, then call `run_sync_locked`.
pub fn run_sync_locked_from(start: &Path) -> Result<SyncReport, PkgError> {
    let root = find_project_root(start).ok_or(PkgError::NoProject)?;
    run_sync_locked(&root)
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
        // For branch deps, look up the resolved rev from the lockfile.
        let resolved_rev: Option<String> = if dep.rev.is_some() {
            dep.rev.clone()
        } else {
            let expected_source = format!("git+{url}");
            load_lockfile(project_root).ok().flatten().and_then(|lf| {
                lf.packages
                    .into_iter()
                    .find(|p| p.source == expected_source)
                    .and_then(|p| p.rev)
            })
        };

        out.push_str(&format!("  source:  git {url}\n"));
        if let Some(branch) = &dep.branch {
            out.push_str(&format!("  branch:  {branch}\n"));
        }
        let rev_display = resolved_rev.as_deref().unwrap_or("?");
        out.push_str(&format!("  rev:     {rev_display}\n"));
        if let Some(rev) = &resolved_rev {
            let cache = cache_dir_for(dep_name, url, rev);
            let synced = cache.join("Tyra.toml").is_file();
            out.push_str(&format!("  cache:   {}\n", cache.display()));
            out.push_str(&format!(
                "  synced:  {}\n",
                if synced { "yes" } else { "no" }
            ));
        } else {
            out.push_str("  synced:  no (not yet synced)\n");
        }
    }

    Ok(out)
}

/// Locate the project root walking up from `start`, then call `run_show`.
pub fn run_show_from(start: &Path, dep_name: &str) -> Result<String, PkgError> {
    let root = find_project_root(start).ok_or(PkgError::NoProject)?;
    run_show(&root, dep_name)
}

/// `tyra mod show <dep_name> --json`
///
/// Returns a JSON object with the dependency's resolved metadata.
pub fn run_show_json(project_root: &Path, dep_name: &str) -> Result<String, PkgError> {
    let manifest = load_manifest(project_root)?;
    let dep = manifest
        .dependencies
        .get(dep_name)
        .ok_or_else(|| PkgError::DepNotFound(dep_name.to_string()))?;

    if let Some(path_str) = &dep.path {
        let abs = project_root.join(path_str);
        let (pkg_name, version) = match load_manifest(&abs) {
            Ok(m) => (m.package.name.clone(), m.package.version.clone()),
            Err(_) => (String::new(), String::new()),
        };
        Ok(format!(
            "{{\n  \"name\": {},\n  \"source\": \"path\",\n  \"path\": {},\n  \"root\": {},\n  \"package_name\": {},\n  \"version\": {}\n}}\n",
            json_str(dep_name),
            json_str(path_str),
            json_str(&abs.to_string_lossy()),
            json_str(&pkg_name),
            json_str(&version),
        ))
    } else if let Some(url) = &dep.git {
        // For branch deps, look up the resolved rev from the lockfile.
        let resolved_rev: Option<String> = if dep.rev.is_some() {
            dep.rev.clone()
        } else {
            let expected_source = format!("git+{url}");
            load_lockfile(project_root).ok().flatten().and_then(|lf| {
                lf.packages
                    .into_iter()
                    .find(|p| p.source == expected_source)
                    .and_then(|p| p.rev)
            })
        };
        let rev_str = resolved_rev.as_deref().unwrap_or("?");
        let (cache_str, synced) = if let Some(rev) = &resolved_rev {
            let cache = cache_dir_for(dep_name, url, rev);
            let synced = cache.join("Tyra.toml").is_file();
            (cache.to_string_lossy().into_owned(), synced)
        } else {
            (String::new(), false)
        };
        let branch_field = if let Some(b) = &dep.branch {
            format!(",\n  \"branch\": {}", json_str(b))
        } else {
            String::new()
        };
        Ok(format!(
            "{{\n  \"name\": {},\n  \"source\": \"git\",\n  \"url\": {}{},\n  \"rev\": {},\n  \"cache\": {},\n  \"synced\": {}\n}}\n",
            json_str(dep_name),
            json_str(url),
            branch_field,
            json_str(rev_str),
            json_str(&cache_str),
            if synced { "true" } else { "false" },
        ))
    } else {
        Err(PkgError::DepNotFound(dep_name.to_string()))
    }
}

/// Locate the project root walking up from `start`, then call `run_show_json`.
pub fn run_show_json_from(start: &Path, dep_name: &str) -> Result<String, PkgError> {
    let root = find_project_root(start).ok_or(PkgError::NoProject)?;
    run_show_json(&root, dep_name)
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
        match (&dep.path, &dep.git) {
            (Some(rel), _) => {
                let dep_root = project_root.join(rel);
                if let Err(e) = validate_dep_root(dep_name, &dep_root) {
                    issues.push(format!("{dep_name}: {e}"));
                }
            }
            (None, Some(url)) => {
                if let Some(rev) = &dep.rev {
                    let cache_dir = cache_dir_for(dep_name, url, rev);
                    if !cache_dir.join("Tyra.toml").is_file() {
                        issues.push(format!("{dep_name}: not synced (run `tyra mod sync`)"));
                    } else if let Err(e) = validate_dep_root(dep_name, &cache_dir) {
                        issues.push(format!("{dep_name}: {e}"));
                    }
                } else {
                    // branch dep — check lockfile for resolved rev (match by git source)
                    let expected_source = format!("git+{url}");
                    match load_lockfile(project_root) {
                        Ok(Some(lf)) => {
                            if let Some(pkg) =
                                lf.packages.iter().find(|p| p.source == expected_source)
                            {
                                if let Some(rev) = &pkg.rev {
                                    let cache_dir = cache_dir_for(dep_name, url, rev);
                                    if !cache_dir.join("Tyra.toml").is_file() {
                                        issues.push(format!(
                                            "{dep_name}: not synced (run `tyra mod sync`)"
                                        ));
                                    } else if let Err(e) = validate_dep_root(dep_name, &cache_dir) {
                                        issues.push(format!("{dep_name}: {e}"));
                                    }
                                }
                            } else {
                                issues.push(format!(
                                    "{dep_name}: not in Tyra.lock (run `tyra mod sync`)"
                                ));
                            }
                        }
                        Ok(None) => {
                            issues.push(format!(
                                "{dep_name}: branch dep requires Tyra.lock — run `tyra mod sync`"
                            ));
                        }
                        Err(e) => issues.push(format!("{dep_name}: {e}")),
                    }
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
    home.join(".tyra")
        .join("cache")
        .join("git")
        .join(dir_name)
        .join(rev)
}

/// Resolve `..` and `.` segments without requiring the path to exist.
fn normalize_path(path: &Path) -> PathBuf {
    use std::path::Component;
    let mut comps: Vec<Component<'_>> = Vec::new();
    for c in path.components() {
        match c {
            Component::CurDir => {}
            Component::ParentDir => {
                if matches!(comps.last(), Some(Component::Normal(_))) {
                    comps.pop();
                } else {
                    comps.push(c);
                }
            }
            other => comps.push(other),
        }
    }
    comps.iter().collect()
}

/// Relative path from `base` to `path` (both should be absolute or share a common root).
fn path_relative_to(path: &Path, base: &Path) -> PathBuf {
    let path = normalize_path(path);
    let base = normalize_path(base);
    let p: Vec<_> = path.components().collect();
    let b: Vec<_> = base.components().collect();
    let common = p.iter().zip(b.iter()).take_while(|(a, c)| a == c).count();
    let mut result = PathBuf::new();
    for _ in 0..(b.len() - common) {
        result.push("..");
    }
    for comp in &p[common..] {
        result.push(comp);
    }
    if result.as_os_str().is_empty() {
        result.push(".");
    }
    result
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

impl SyncReport {
    pub fn to_json(&self) -> String {
        let arr = |v: &[String]| -> String {
            let items: Vec<String> = v.iter().map(|s| json_str(s)).collect();
            format!("[{}]", items.join(", "))
        };
        format!(
            "{{\n  \"synced\": {},\n  \"cached\": {},\n  \"skipped\": {}\n}}\n",
            arr(&self.synced),
            arr(&self.cached),
            arr(&self.skipped),
        )
    }
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
// Transitive resolver
// ---------------------------------------------------------------------------

/// Internal record of a resolved dependency (git or path).
struct ResolvedEntry {
    name: String,
    source: String,
    rev: Option<String>,
    /// Branch constraint that was resolved to `rev` (branch deps only).
    branch: Option<String>,
    pkg_version: Option<String>,
}

/// Walk the local path-dep tree from `dep_root` and collect the canonical source
/// string (`path+<project-root-relative>`) for every reachable path dep.
///
/// Uses `original_root` to normalise each dep's path relative to the project root,
/// matching how `resolve_transitive_impl` writes sources to the lockfile.
/// Only follows `path` edges; git dep sub-graphs are pinned by rev and opaque here.
fn collect_path_dep_sources(
    dep_root: &Path,
    original_root: &Path,
    out: &mut HashSet<String>,
    depth: u32,
) -> Result<(), PkgError> {
    if depth > 32 {
        return Ok(());
    }
    let manifest = load_manifest(dep_root)?;
    for dep in manifest.dependencies.values() {
        if let Some(rel) = &dep.path {
            let child_root = normalize_path(&dep_root.join(rel));
            let source = format!(
                "path+{}",
                path_relative_to(&child_root, original_root).display()
            );
            if out.insert(source) {
                collect_path_dep_sources(&child_root, original_root, out, depth + 1)?;
            }
        }
    }
    Ok(())
}

/// Recursively resolve all dependencies reachable from `project_root`.
///
/// # Key scheme (Issue 2 fix)
///
/// `resolved` is keyed by **canonical source**, not by the local dep alias:
/// - path dep → absolute (normalised) dep root path as string
/// - git dep  → bare git URL (without rev)
///
/// This prevents two manifests that use the same local alias (e.g. `utils`)
/// for different packages from silently colliding or generating false conflicts.
///
/// # Path normalisation (Issue 1 fix)
///
/// Path dep sources are stored as `path+<relative-to-original-root>` so that
/// `--locked` validation can always join relative to the top-level project root
/// regardless of which sub-manifest declared the dependency.
///
/// A depth limit of 32 guards against accidental cycles.
fn resolve_transitive_impl(
    project_root: &Path,
    root: &Path,
    resolved: &mut HashMap<String, ResolvedEntry>,
    report: &mut SyncReport,
    depth: u32,
    original_root: &Path,
) -> Result<(), PkgError> {
    if depth > 32 {
        return Err(PkgError::SyncFailed {
            dep: String::new(),
            message: "dependency graph exceeds depth limit of 32 (possible cycle)".to_string(),
        });
    }
    let manifest = load_manifest(project_root)?;
    let mut deps: Vec<(&String, &Dependency)> = manifest.dependencies.iter().collect();
    deps.sort_by_key(|(k, _)| k.as_str());

    for (dep_name, dep) in deps {
        match (&dep.path, &dep.git) {
            (Some(rel), _) => {
                let dep_root = normalize_path(&root.join(rel));
                // Canonical key = absolute dep root path (prevents alias collisions).
                let canonical_key = dep_root.to_string_lossy().to_string();
                if !resolved.contains_key(&canonical_key) {
                    // Source is relative to the original project root so that
                    // `--locked` validation always joins relative to the same base.
                    let source = format!(
                        "path+{}",
                        path_relative_to(&dep_root, original_root).display()
                    );
                    if let Some(existing) = resolved.values().find(|r| r.name == *dep_name) {
                        return Err(PkgError::DepNameCollision {
                            name: dep_name.clone(),
                            source_existing: existing.source.clone(),
                            source_new: source.clone(),
                        });
                    }
                    // ADR 0009/0010: validate package.name == dep_key, src/<name>.tyra
                    // exists, and the dep is not a bin package.
                    validate_dep_root(dep_name, &dep_root)?;
                    let pkg_version = load_manifest(&dep_root)
                        .ok()
                        .map(|m| m.package.version.clone());
                    resolved.insert(
                        canonical_key,
                        ResolvedEntry {
                            name: dep_name.clone(),
                            source,
                            rev: None,
                            branch: None,
                            pkg_version,
                        },
                    );
                    if depth == 0 {
                        report.skipped.push(dep_name.clone());
                    }
                    resolve_transitive_impl(
                        &dep_root,
                        &dep_root,
                        resolved,
                        report,
                        depth + 1,
                        original_root,
                    )?;
                } else if resolved[&canonical_key].name != *dep_name {
                    // Same source already resolved under a different alias — ADR 0009/0010
                    // requires dep_key == package.name, so this is a no-aliasing violation.
                    return Err(PkgError::NameMismatch {
                        key: dep_name.clone(),
                        package_name: resolved[&canonical_key].name.clone(),
                    });
                }
            }
            (None, Some(url)) => {
                // Resolve branch → exact SHA if needed.
                let (rev, branch) = if let Some(r) = &dep.rev {
                    (r.clone(), None)
                } else if let Some(b) = &dep.branch {
                    (git_resolve_branch(dep_name, url, b)?, Some(b.clone()))
                } else {
                    continue; // validated already, unreachable
                };

                // Canonical key = git URL (same URL from different aliases = same package).
                if let Some(existing) = resolved.get(url.as_str()) {
                    // ADR 0009/0010: dep_key must equal package.name; the same git URL
                    // must not be referenced under a different alias.
                    if existing.name != *dep_name {
                        return Err(PkgError::NameMismatch {
                            key: dep_name.clone(),
                            package_name: existing.name.clone(),
                        });
                    }
                    if existing.rev.as_deref() != Some(rev.as_str()) {
                        return Err(PkgError::DepConflict {
                            name: format!("{dep_name} ({url})"),
                            rev_existing: existing.rev.clone().unwrap_or_default(),
                            rev_new: rev,
                        });
                    }
                    // Same rev but different branch constraints: even if the SHA matches
                    // today, the two branches may diverge later.  Require a single
                    // consistent constraint across the whole dep graph.
                    if existing.branch.as_deref() != branch.as_deref() {
                        let describe = |b: Option<&str>, r: &str| match b {
                            Some(br) => format!("branch={br} ({r})"),
                            None => format!("rev={r}"),
                        };
                        return Err(PkgError::DepConflict {
                            name: format!("{dep_name} ({url})"),
                            rev_existing: describe(
                                existing.branch.as_deref(),
                                existing.rev.as_deref().unwrap_or("?"),
                            ),
                            rev_new: describe(branch.as_deref(), &rev),
                        });
                    }
                    continue; // same rev and same constraint — already resolved
                }

                let status = sync_git_dep(dep_name, url, &rev)?;
                if depth == 0 {
                    match status {
                        SyncStatus::Fresh => report.synced.push(dep_name.clone()),
                        SyncStatus::Cached => report.cached.push(dep_name.clone()),
                    }
                }

                let cache_dir = cache_dir_for(dep_name, url, &rev);
                let pkg_version = load_manifest(&cache_dir)
                    .ok()
                    .map(|m| m.package.version.clone());

                let git_source = format!("git+{url}");
                if let Some(existing) = resolved.values().find(|r| r.name == *dep_name) {
                    return Err(PkgError::DepNameCollision {
                        name: dep_name.clone(),
                        source_existing: existing.source.clone(),
                        source_new: git_source.clone(),
                    });
                }
                resolved.insert(
                    url.clone(),
                    ResolvedEntry {
                        name: dep_name.clone(),
                        source: git_source,
                        rev: Some(rev.clone()),
                        branch,
                        pkg_version,
                    },
                );

                resolve_transitive_impl(
                    &cache_dir,
                    &cache_dir,
                    resolved,
                    report,
                    depth + 1,
                    original_root,
                )?;
            }
            _ => {}
        }
    }
    Ok(())
}

/// Resolve a branch name to an exact commit SHA using `git ls-remote`.
///
/// Returns the 40-character SHA string.
fn git_resolve_branch(dep_name: &str, url: &str, branch: &str) -> Result<String, PkgError> {
    let refspec = format!("refs/heads/{branch}");
    let output = Command::new("git")
        .args(["ls-remote", url, &refspec])
        .output()
        .map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                PkgError::GitNotAvailable
            } else {
                PkgError::Io(e)
            }
        })?;

    if !output.status.success() {
        return Err(PkgError::SyncFailed {
            dep: dep_name.to_string(),
            message: format!("git ls-remote failed for `{url}` branch `{branch}`"),
        });
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    // ls-remote output: "<sha>\trefs/heads/<branch>\n"
    if let Some(sha) = stdout.split_whitespace().next()
        && sha.len() >= 7
    {
        return Ok(sha.to_string());
    }

    Err(PkgError::SyncFailed {
        dep: dep_name.to_string(),
        message: format!("branch `{branch}` not found in `{url}`"),
    })
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
        .map_err(PkgError::Io)?;
    if !checkout_status.success() {
        let _ = std::fs::remove_dir_all(&tmp_dir);
        return Err(PkgError::SyncFailed {
            dep: dep_name.to_string(),
            message: format!("git checkout `{rev}` failed"),
        });
    }

    // Validate ADR 0009/0010 invariants before committing to cache.
    validate_dep_root(dep_name, &tmp_dir).inspect_err(|_| {
        let _ = std::fs::remove_dir_all(&tmp_dir);
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
        && name
            .chars()
            .next()
            .map(|c| c.is_ascii_lowercase())
            .unwrap_or(false)
        && name
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
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

/// Replace the entry for `dep_name` in the `[dependencies]` section in-place.
/// Preserves the surrounding lines and the original position of the entry.
fn replace_dependency_line(content: &str, dep_name: &str, new_line: &str) -> String {
    let trailing_newline = content.ends_with('\n');
    let result: Vec<&str> = content
        .lines()
        .map(|l| {
            let t = l.trim_start();
            if t.starts_with(dep_name) && t[dep_name.len()..].trim_start().starts_with('=') {
                new_line
            } else {
                l
            }
        })
        .collect();
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
            !(t.starts_with(dep_name) && t[dep_name.len()..].trim_start().starts_with('='))
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
                let mut sub_deps: Vec<(&String, &Dependency)> = m.dependencies.iter().collect();
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
            format!("[package]\nname    = \"{name}\"\nversion = \"0.1.0\"\nedition = \"2026\"\n"),
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
        assert!(
            !tree.contains("[cycle]"),
            "diamond DAG must not be flagged as cycle:\n{tree}"
        );
        assert_eq!(
            tree.matches("common").count(),
            2,
            "common should appear twice:\n{tree}"
        );
    }

    #[test]
    fn resolver_rejects_same_name_different_source() {
        // app -> pkg_a -> shared  (source = shared_a)
        // app -> pkg_b -> shared  (source = shared_b, different path, same package.name)
        // → DepNameCollision: both claim import name "shared"
        let dir_shared_a = tempfile::tempdir().unwrap();
        let dir_shared_b = tempfile::tempdir().unwrap();
        let dir_a = tempfile::tempdir().unwrap();
        let dir_b = tempfile::tempdir().unwrap();
        let dir_app = tempfile::tempdir().unwrap();

        make_manifest(dir_shared_a.path(), "shared");
        make_src_file(
            dir_shared_a.path(),
            "shared",
            "export fn hello() -> String\n  \"a\"\nend\n",
        );
        make_manifest(dir_shared_b.path(), "shared");
        // shared_b does not need a src file — DepNameCollision fires before validation.

        make_manifest(dir_a.path(), "pkg_a");
        make_src_file(
            dir_a.path(),
            "pkg_a",
            "export fn hello() -> String\n  \"a\"\nend\n",
        );
        fs::write(
            dir_a.path().join("Tyra.toml"),
            format!(
                "[package]\nname    = \"pkg_a\"\nversion = \"0.1.0\"\nedition = \"2026\"\n\
                 \n[dependencies]\nshared = {{ path = \"{}\" }}\n",
                dir_shared_a.path().display()
            ),
        )
        .unwrap();
        make_manifest(dir_b.path(), "pkg_b");
        make_src_file(
            dir_b.path(),
            "pkg_b",
            "export fn hello() -> String\n  \"b\"\nend\n",
        );
        fs::write(
            dir_b.path().join("Tyra.toml"),
            format!(
                "[package]\nname    = \"pkg_b\"\nversion = \"0.1.0\"\nedition = \"2026\"\n\
                 \n[dependencies]\nshared = {{ path = \"{}\" }}\n",
                dir_shared_b.path().display()
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

        let result = run_sync(dir_app.path());
        assert!(
            matches!(result, Err(PkgError::DepNameCollision { .. })),
            "expected DepNameCollision, got: {result:?}"
        );
    }

    #[test]
    fn run_sync_path_dep_name_mismatch_is_error() {
        // Manifest declares dep key "utils" but the dep's package.name is "mylib".
        // run_sync must reject this with NameMismatch (ADR 0009/0010 invariant).
        let app_dir = tempfile::tempdir().unwrap();
        let lib_dir = tempfile::tempdir().unwrap();
        make_manifest(lib_dir.path(), "mylib");
        make_src_file(
            lib_dir.path(),
            "mylib",
            "export fn greet() -> String\n  \"hi\"\nend\n",
        );
        fs::write(
            app_dir.path().join("Tyra.toml"),
            format!(
                "[package]\nname    = \"app\"\nversion = \"0.1.0\"\nedition = \"2026\"\n\
                 \n[dependencies]\nutils = {{ path = \"{}\" }}\n",
                lib_dir.path().display()
            ),
        )
        .unwrap();
        let result = run_sync(app_dir.path());
        assert!(
            matches!(result, Err(PkgError::NameMismatch { .. })),
            "expected NameMismatch, got: {result:?}"
        );
    }

    #[test]
    fn run_sync_path_dep_same_source_different_alias_is_error() {
        // app -> lib (first visit under dep key "lib")
        // app -> alias (second visit to same path under dep key "alias")
        // → NameMismatch: ADR 0009/0010 no-aliasing violation
        let app_dir = tempfile::tempdir().unwrap();
        let lib_dir = tempfile::tempdir().unwrap();
        make_manifest(lib_dir.path(), "lib");
        make_src_file(
            lib_dir.path(),
            "lib",
            "export fn greet() -> String\n  \"hi\"\nend\n",
        );
        fs::write(
            app_dir.path().join("Tyra.toml"),
            format!(
                "[package]\nname    = \"app\"\nversion = \"0.1.0\"\nedition = \"2026\"\n\
                 \n[dependencies]\nalias = {{ path = \"{}\" }}\nlib = {{ path = \"{}\" }}\n",
                lib_dir.path().display(),
                lib_dir.path().display()
            ),
        )
        .unwrap();
        let result = run_sync(app_dir.path());
        assert!(
            matches!(result, Err(PkgError::NameMismatch { .. })),
            "expected NameMismatch, got: {result:?}"
        );
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
        make_src_file(
            dir.path(),
            "mylib",
            "export fn greet(name: String) -> String\n  name\nend\n",
        );
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
        make_src_file(
            dir.path(),
            "myapp",
            "fn main() -> Unit\n  print(\"hi\")\nend\n",
        );
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
        let content = "[package]\nname    = \"x\"\nversion = \"0.1.0\"\nedition = \"2026\"\n";
        let result = insert_dependency_line(content, "foo = { path = \"../foo\" }");
        assert!(result.contains("[dependencies]"));
        assert!(result.contains("foo = { path = \"../foo\" }"));
    }

    #[test]
    fn insert_appends_within_existing_section() {
        let content = "[package]\nname    = \"x\"\nversion = \"0.1.0\"\nedition = \"2026\"\n\
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

    // --- run_update ---

    #[test]
    fn update_dep_changes_path_in_place() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(
            dir.path().join("Tyra.toml"),
            "[package]\nname    = \"myapp\"\nversion = \"0.1.0\"\nedition = \"2026\"\n\
             \n[dependencies]\nmylib = { path = \"../mylib\" }\nutils = { path = \"../utils\" }\n",
        )
        .unwrap();
        run_update(dir.path(), "mylib", DepSource::Path("../newlib".into())).unwrap();
        let content = fs::read_to_string(dir.path().join("Tyra.toml")).unwrap();
        assert!(
            content.contains("mylib = { path = \"../newlib\" }"),
            "path must be updated"
        );
        assert!(
            content.contains("utils = { path = \"../utils\" }"),
            "utils must be unchanged"
        );
    }

    #[test]
    fn update_nonexistent_dep_is_error() {
        let dir = tempfile::tempdir().unwrap();
        make_manifest(dir.path(), "myapp");
        let result = run_update(dir.path(), "nonexistent", DepSource::Path("../x".into()));
        assert!(matches!(result, Err(PkgError::DepNotFound(_))));
    }

    #[test]
    fn update_dep_preserves_order() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(
            dir.path().join("Tyra.toml"),
            "[package]\nname    = \"myapp\"\nversion = \"0.1.0\"\nedition = \"2026\"\n\
             \n[dependencies]\nalpha = { path = \"../alpha\" }\nbeta = { path = \"../beta\" }\n",
        )
        .unwrap();
        run_update(dir.path(), "alpha", DepSource::Path("../alpha2".into())).unwrap();
        let content = fs::read_to_string(dir.path().join("Tyra.toml")).unwrap();
        let alpha_pos = content.find("alpha2").unwrap();
        let beta_pos = content.find("beta").unwrap();
        assert!(alpha_pos < beta_pos, "alpha must still come before beta");
    }

    // --- replace_dependency_line (unit) ---

    #[test]
    fn replace_line_updates_the_matching_entry() {
        let content = "[package]\nname    = \"x\"\nversion = \"0.1.0\"\nedition = \"2026\"\n\
             \n[dependencies]\nfoo = { path = \"../foo\" }\nbar = { path = \"../bar\" }\n";
        let result = replace_dependency_line(content, "foo", "foo = { path = \"../newfoo\" }");
        assert!(result.contains("foo = { path = \"../newfoo\" }"));
        assert!(result.contains("bar = { path = \"../bar\" }"));
        assert!(!result.contains("../foo\""), "old path must be gone");
    }

    // --- remove_dependency_line (unit) ---

    #[test]
    fn remove_line_leaves_section_header() {
        let content = "[package]\nname    = \"x\"\nversion = \"0.1.0\"\nedition = \"2026\"\n\
             \n[dependencies]\nalpha = { path = \"../alpha\" }\n";
        let result = remove_dependency_line(content, "alpha");
        assert!(result.contains("[dependencies]"), "header must remain");
        assert!(!result.contains("alpha = "));
    }

    #[test]
    fn remove_line_preserves_others() {
        let content = "[package]\nname    = \"x\"\nversion = \"0.1.0\"\nedition = \"2026\"\n\
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
        fs::write(
            src.join("mylib.tyra"),
            "export fn greet() -> String\n  \"hi\"\nend\n",
        )
        .unwrap();
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
        assert!(
            json.contains("\"name\":\"myapp\""),
            "missing name field: {json}"
        );
        assert!(
            json.contains("\"version\":\"0.1.0\""),
            "missing version field: {json}"
        );
        assert!(
            json.contains("\"deps\":[]"),
            "empty deps array missing: {json}"
        );
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
                    if depth == 0 {
                        break;
                    }
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
        assert_eq!(
            top_level_keys, 1,
            "root.deps must have exactly 1 direct element: {json}"
        );
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
        assert!(
            json.contains("\"cycle\":true"),
            "cycle node missing: {json}"
        );
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
        assert!(
            json.contains("\"synced\":false"),
            "git dep must report synced:false: {json}"
        );
        assert!(
            json.contains("\"key\":\"utils\""),
            "utils key missing: {json}"
        );
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
        assert!(
            !json.contains("\"cycle\":true"),
            "diamond must not be flagged as cycle: {json}"
        );
        assert_eq!(
            json.matches("\"key\":\"common\"").count(),
            2,
            "common must appear twice: {json}"
        );
    }

    // --- run_sync_locked ---

    #[test]
    fn locked_no_lockfile_is_error() {
        let dir = tempfile::tempdir().unwrap();
        make_manifest(dir.path(), "myapp");
        let result = run_sync_locked(dir.path());
        assert!(
            matches!(result, Err(PkgError::LockfileNotFound)),
            "expected LockfileNotFound, got: {result:?}"
        );
    }

    #[test]
    fn locked_passes_with_valid_path_dep() {
        // Layout: base/proj/ (project), base/lib/ (dep "mylib")
        // manifest: mylib = { path = "../lib" }
        // lockfile: source = "path+../lib"
        let base = tempfile::tempdir().unwrap();
        let proj = base.path().join("proj");
        let lib_dir = base.path().join("lib");
        fs::create_dir_all(&proj).unwrap();
        fs::create_dir_all(&lib_dir).unwrap();

        fs::write(
            proj.join("Tyra.toml"),
            "[package]\nname    = \"myapp\"\nversion = \"0.1.0\"\nedition = \"2026\"\n\
             \n[dependencies]\nmylib = { path = \"../lib\" }\n",
        )
        .unwrap();
        make_manifest(&lib_dir, "mylib");
        make_src_file(
            &lib_dir,
            "mylib",
            "export fn greet() -> String\n  \"hi\"\nend\n",
        );

        let pkgs = vec![LockedPackage {
            name: "mylib".into(),
            source: "path+../lib".into(),
            rev: None,
            branch: None,
            pkg_version: Some("0.1.0".into()),
        }];
        build_and_write_lockfile(&proj, pkgs).unwrap();

        let result = run_sync_locked(&proj);
        assert!(result.is_ok(), "expected Ok, got: {result:?}");
    }

    #[test]
    fn locked_detects_source_url_change() {
        let dir = tempfile::tempdir().unwrap();
        // Manifest: new URL
        fs::write(
            dir.path().join("Tyra.toml"),
            "[package]\nname    = \"myapp\"\nversion = \"0.1.0\"\nedition = \"2026\"\n\
             \n[dependencies]\nutils = { git = \"https://github.com/example/NEW.git\", \
             rev = \"abc1234\" }\n",
        )
        .unwrap();
        // Lockfile: old URL
        let pkgs = vec![LockedPackage {
            name: "utils".into(),
            source: "git+https://github.com/example/OLD.git".into(),
            rev: Some("abc1234".into()),
            branch: None,
            pkg_version: None,
        }];
        build_and_write_lockfile(dir.path(), pkgs).unwrap();

        let result = run_sync_locked(dir.path());
        assert!(
            matches!(result, Err(PkgError::LockfileOutOfSync(_))),
            "expected LockfileOutOfSync (url change), got: {result:?}"
        );
    }

    #[test]
    fn locked_detects_dep_key_rename() {
        let dir = tempfile::tempdir().unwrap();
        // Manifest: dep key "renamed_lib" with same URL
        fs::write(
            dir.path().join("Tyra.toml"),
            "[package]\nname    = \"myapp\"\nversion = \"0.1.0\"\nedition = \"2026\"\n\
             \n[dependencies]\nrenamed_lib = { git = \"https://github.com/example/lib.git\", \
             rev = \"abc1234\" }\n",
        )
        .unwrap();
        // Lockfile: was named "mylib" (original key before rename)
        let pkgs = vec![LockedPackage {
            name: "mylib".into(),
            source: "git+https://github.com/example/lib.git".into(),
            rev: Some("abc1234".into()),
            branch: None,
            pkg_version: None,
        }];
        build_and_write_lockfile(dir.path(), pkgs).unwrap();

        let result = run_sync_locked(dir.path());
        assert!(
            matches!(result, Err(PkgError::LockfileOutOfSync(_))),
            "expected LockfileOutOfSync (dep key rename), got: {result:?}"
        );
    }

    #[test]
    fn locked_detects_rev_change() {
        let dir = tempfile::tempdir().unwrap();
        // Manifest: rev = "newrev"
        fs::write(
            dir.path().join("Tyra.toml"),
            "[package]\nname    = \"myapp\"\nversion = \"0.1.0\"\nedition = \"2026\"\n\
             \n[dependencies]\nutils = { git = \"https://github.com/example/utils.git\", \
             rev = \"newrev\" }\n",
        )
        .unwrap();
        // Lockfile: rev = "oldrev"
        let pkgs = vec![LockedPackage {
            name: "utils".into(),
            source: "git+https://github.com/example/utils.git".into(),
            rev: Some("oldrev".into()),
            branch: None,
            pkg_version: None,
        }];
        build_and_write_lockfile(dir.path(), pkgs).unwrap();

        let result = run_sync_locked(dir.path());
        assert!(
            matches!(result, Err(PkgError::LockfileOutOfSync(_))),
            "expected LockfileOutOfSync (rev change), got: {result:?}"
        );
    }

    #[test]
    fn locked_detects_branch_name_change() {
        let dir = tempfile::tempdir().unwrap();
        // Manifest: branch = "develop"
        fs::write(
            dir.path().join("Tyra.toml"),
            "[package]\nname    = \"myapp\"\nversion = \"0.1.0\"\nedition = \"2026\"\n\
             \n[dependencies]\nutils = { git = \"https://github.com/example/utils.git\", \
             branch = \"develop\" }\n",
        )
        .unwrap();
        // Lockfile: branch = "main"
        let pkgs = vec![LockedPackage {
            name: "utils".into(),
            source: "git+https://github.com/example/utils.git".into(),
            rev: Some("abc1234".into()),
            branch: Some("main".into()),
            pkg_version: None,
        }];
        build_and_write_lockfile(dir.path(), pkgs).unwrap();

        let result = run_sync_locked(dir.path());
        assert!(
            matches!(result, Err(PkgError::LockfileOutOfSync(_))),
            "expected LockfileOutOfSync (branch name change), got: {result:?}"
        );
    }

    #[test]
    fn locked_detects_branch_to_rev_constraint_change() {
        let dir = tempfile::tempdir().unwrap();
        // Manifest: now pinned rev (was floating branch)
        fs::write(
            dir.path().join("Tyra.toml"),
            "[package]\nname    = \"myapp\"\nversion = \"0.1.0\"\nedition = \"2026\"\n\
             \n[dependencies]\nutils = { git = \"https://github.com/example/utils.git\", \
             rev = \"abc1234\" }\n",
        )
        .unwrap();
        // Lockfile: was floating branch
        let pkgs = vec![LockedPackage {
            name: "utils".into(),
            source: "git+https://github.com/example/utils.git".into(),
            rev: Some("abc1234".into()),
            branch: Some("main".into()),
            pkg_version: None,
        }];
        build_and_write_lockfile(dir.path(), pkgs).unwrap();

        let result = run_sync_locked(dir.path());
        assert!(
            matches!(result, Err(PkgError::LockfileOutOfSync(_))),
            "expected LockfileOutOfSync (branch→rev), got: {result:?}"
        );
    }

    #[test]
    fn locked_detects_rev_to_branch_constraint_change() {
        let dir = tempfile::tempdir().unwrap();
        // Manifest: now floating branch (was pinned rev)
        fs::write(
            dir.path().join("Tyra.toml"),
            "[package]\nname    = \"myapp\"\nversion = \"0.1.0\"\nedition = \"2026\"\n\
             \n[dependencies]\nutils = { git = \"https://github.com/example/utils.git\", \
             branch = \"main\" }\n",
        )
        .unwrap();
        // Lockfile: was pinned rev, no branch field
        let pkgs = vec![LockedPackage {
            name: "utils".into(),
            source: "git+https://github.com/example/utils.git".into(),
            rev: Some("abc1234".into()),
            branch: None,
            pkg_version: None,
        }];
        build_and_write_lockfile(dir.path(), pkgs).unwrap();

        let result = run_sync_locked(dir.path());
        assert!(
            matches!(result, Err(PkgError::LockfileOutOfSync(_))),
            "expected LockfileOutOfSync (rev→branch), got: {result:?}"
        );
    }

    #[test]
    fn locked_detects_new_transitive_path_dep() {
        // proj → lib → child  (child is new — not in lockfile)
        let base = tempfile::tempdir().unwrap();
        let proj = base.path().join("proj");
        let lib_dir = base.path().join("lib");
        let child_dir = base.path().join("child");
        fs::create_dir_all(&proj).unwrap();
        fs::create_dir_all(&lib_dir).unwrap();
        fs::create_dir_all(&child_dir).unwrap();

        make_manifest(&child_dir, "child");
        make_src_file(&child_dir, "child", "export fn noop() -> Unit\nend\n");

        fs::write(
            lib_dir.join("Tyra.toml"),
            format!(
                "[package]\nname    = \"mylib\"\nversion = \"0.1.0\"\nedition = \"2026\"\n\
                 \n[dependencies]\nchild = {{ path = \"../child\" }}\n"
            ),
        )
        .unwrap();
        make_src_file(
            &lib_dir,
            "mylib",
            "export fn greet() -> String\n  \"hi\"\nend\n",
        );

        fs::write(
            proj.join("Tyra.toml"),
            "[package]\nname    = \"myapp\"\nversion = \"0.1.0\"\nedition = \"2026\"\n\
             \n[dependencies]\nmylib = { path = \"../lib\" }\n",
        )
        .unwrap();

        // Lockfile only records mylib — child is missing
        let pkgs = vec![LockedPackage {
            name: "mylib".into(),
            source: "path+../lib".into(),
            rev: None,
            branch: None,
            pkg_version: Some("0.1.0".into()),
        }];
        build_and_write_lockfile(&proj, pkgs).unwrap();

        let result = run_sync_locked(&proj);
        assert!(
            matches!(result, Err(PkgError::LockfileOutOfSync(_))),
            "expected LockfileOutOfSync (new transitive dep), got: {result:?}"
        );
    }

    #[test]
    fn locked_detects_removed_transitive_path_dep() {
        // proj → lib  (child was removed from lib, but lockfile still records it)
        let base = tempfile::tempdir().unwrap();
        let proj = base.path().join("proj");
        let lib_dir = base.path().join("lib");
        fs::create_dir_all(&proj).unwrap();
        fs::create_dir_all(&lib_dir).unwrap();

        // lib: no deps now
        make_manifest(&lib_dir, "mylib");
        make_src_file(
            &lib_dir,
            "mylib",
            "export fn greet() -> String\n  \"hi\"\nend\n",
        );

        fs::write(
            proj.join("Tyra.toml"),
            "[package]\nname    = \"myapp\"\nversion = \"0.1.0\"\nedition = \"2026\"\n\
             \n[dependencies]\nmylib = { path = \"../lib\" }\n",
        )
        .unwrap();

        // Lockfile still has stale "child" entry
        let pkgs = vec![
            LockedPackage {
                name: "mylib".into(),
                source: "path+../lib".into(),
                rev: None,
                branch: None,
                pkg_version: Some("0.1.0".into()),
            },
            LockedPackage {
                name: "child".into(),
                source: "path+../child".into(),
                rev: None,
                branch: None,
                pkg_version: None,
            },
        ];
        build_and_write_lockfile(&proj, pkgs).unwrap();

        let result = run_sync_locked(&proj);
        assert!(
            matches!(result, Err(PkgError::LockfileOutOfSync(_))),
            "expected LockfileOutOfSync (removed transitive dep), got: {result:?}"
        );
    }
}
