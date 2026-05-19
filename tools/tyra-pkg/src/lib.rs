//! tyra-pkg: dependency management commands for the Tyra language.
//!
//! Public API:
//! - `run_init(dest, name)` — create Tyra.toml in an existing directory
//! - `run_add(project_root, dep_name, source)` — append a dependency entry
//! - `run_tree(project_root)` — render the dependency tree as a string

use std::collections::HashSet;
use std::path::{Path, PathBuf};
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
    /// Package or dep name violates naming rules.
    InvalidName(String),
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
            PkgError::InvalidName(n) => write!(
                f,
                "invalid name `{n}`: must start with a lowercase letter, \
                 contain only lowercase letters, digits, and underscores, \
                 and must not be a reserved word"
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

/// Locate the project root walking up from `start`, then call `run_add`.
pub fn run_add_from(
    start: &Path,
    dep_name: &str,
    source: DepSource,
) -> Result<(), PkgError> {
    let root = find_project_root(start).ok_or(PkgError::NoProject)?;
    run_add(&root, dep_name, source)
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

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
}
