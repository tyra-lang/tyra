//! tyra-new: project scaffolding for the Tyra language.
//!
//! Public API:
//! - `create_project(name, kind, dest)` — write a new project tree under `dest/<name>/`.

use std::path::Path;

// ---------------------------------------------------------------------------
// Public types
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProjectKind {
    Bin,
    Lib,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VcsMode {
    /// Write `.gitignore` (default).
    Git,
    /// Skip all VCS files.
    None,
}

#[derive(Debug)]
pub enum NewError {
    /// Target directory already exists.
    AlreadyExists(std::path::PathBuf),
    Io(std::io::Error),
    /// Package name is not a valid Tyra identifier.
    InvalidName(String),
}

impl std::fmt::Display for NewError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            NewError::AlreadyExists(p) => {
                write!(f, "directory already exists: {}", p.display())
            }
            NewError::Io(e) => write!(f, "I/O error: {e}"),
            NewError::InvalidName(n) => write!(
                f,
                "invalid package name `{n}`: must start with a lowercase letter and \
                 contain only lowercase letters, digits, and underscores, \
                 and must not be a reserved word"
            ),
        }
    }
}

impl std::error::Error for NewError {}

impl From<std::io::Error> for NewError {
    fn from(e: std::io::Error) -> Self {
        NewError::Io(e)
    }
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Scaffold a new Tyra project.
///
/// Creates `<dest>/<name>/` with:
/// - `Tyra.toml`
/// - `src/<name>.tyra`
/// - `.gitignore` (unless `vcs == VcsMode::None`)
/// - `README.md`
///
/// Fails with `NewError::AlreadyExists` if `<dest>/<name>/` already exists.
pub fn create_project(
    name: &str,
    kind: ProjectKind,
    vcs: VcsMode,
    dest: &Path,
) -> Result<(), NewError> {
    validate_name(name)?;

    let project_dir = dest.join(name);
    if project_dir.exists() {
        return Err(NewError::AlreadyExists(project_dir));
    }

    let src_dir = project_dir.join("src");
    std::fs::create_dir_all(&src_dir)?;

    write_file(&project_dir.join("Tyra.toml"), &render_manifest(name))?;
    write_file(&src_dir.join(format!("{name}.tyra")), render_source(kind))?;
    if vcs == VcsMode::Git {
        write_file(&project_dir.join(".gitignore"), GITIGNORE)?;
    }
    write_file(&project_dir.join("README.md"), &render_readme(name))?;

    Ok(())
}

// ---------------------------------------------------------------------------
// Templates
// ---------------------------------------------------------------------------

fn render_manifest(name: &str) -> String {
    format!("[package]\nname    = \"{name}\"\nversion = \"0.1.0\"\nedition = \"2026\"\n")
}

fn render_source(kind: ProjectKind) -> &'static str {
    match kind {
        ProjectKind::Bin => BIN_SOURCE,
        ProjectKind::Lib => LIB_SOURCE,
    }
}

fn render_readme(name: &str) -> String {
    format!("# {name}\n")
}

const BIN_SOURCE: &str = "\
fn main() -> Unit
  print(\"Hello, Tyra!\\n\")
end
";

const LIB_SOURCE: &str = "\
export fn greet(name: String) -> String
  \"hello, #{name}\"
end
";

const GITIGNORE: &str = "/target\n";

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

const RESERVED_WORDS: &[&str] = &[
    "fn", "data", "value", "type", "trait", "impl", "let", "mut", "if", "else", "match", "when",
    "for", "in", "while", "return", "defer", "async", "await", "spawn", "import", "export", "and",
    "or", "not", "true", "false", "end",
];

fn validate_name(name: &str) -> Result<(), NewError> {
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
        Err(NewError::InvalidName(name.to_string()))
    }
}

fn write_file(path: &Path, content: &str) -> Result<(), NewError> {
    std::fs::write(path, content)?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn scaffold(name: &str, kind: ProjectKind) -> (tempfile::TempDir, std::path::PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        create_project(name, kind, VcsMode::Git, dir.path()).unwrap();
        let proj = dir.path().join(name);
        (dir, proj)
    }

    #[test]
    fn bin_creates_expected_files() {
        let (_dir, proj) = scaffold("myapp", ProjectKind::Bin);
        assert!(proj.join("Tyra.toml").is_file());
        assert!(proj.join("src/myapp.tyra").is_file());
        assert!(proj.join(".gitignore").is_file());
        assert!(proj.join("README.md").is_file());
    }

    #[test]
    fn bin_manifest_contains_name_and_edition() {
        let (_dir, proj) = scaffold("myapp", ProjectKind::Bin);
        let toml = std::fs::read_to_string(proj.join("Tyra.toml")).unwrap();
        assert!(toml.contains("name    = \"myapp\""));
        assert!(toml.contains("edition = \"2026\""));
    }

    #[test]
    fn bin_source_has_fn_main_and_print() {
        let (_dir, proj) = scaffold("myapp", ProjectKind::Bin);
        let src = std::fs::read_to_string(proj.join("src/myapp.tyra")).unwrap();
        assert!(src.contains("fn main() -> Unit"));
        assert!(src.contains("print(\"Hello, Tyra!\\n\")"));
    }

    #[test]
    fn lib_source_has_export_fn_no_main() {
        let (_dir, proj) = scaffold("mylib", ProjectKind::Lib);
        let src = std::fs::read_to_string(proj.join("src/mylib.tyra")).unwrap();
        assert!(src.contains("export fn greet"));
        assert!(!src.contains("fn main"));
    }

    #[test]
    fn readme_contains_name() {
        let (_dir, proj) = scaffold("myapp", ProjectKind::Bin);
        let readme = std::fs::read_to_string(proj.join("README.md")).unwrap();
        assert_eq!(readme, "# myapp\n");
    }

    #[test]
    fn existing_dir_returns_already_exists_error() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join("myapp")).unwrap();
        let result = create_project("myapp", ProjectKind::Bin, VcsMode::Git, dir.path());
        assert!(matches!(result, Err(NewError::AlreadyExists(_))));
    }

    #[test]
    fn invalid_name_empty() {
        let dir = tempfile::tempdir().unwrap();
        let result = create_project("", ProjectKind::Bin, VcsMode::Git, dir.path());
        assert!(matches!(result, Err(NewError::InvalidName(_))));
    }

    #[test]
    fn invalid_name_starts_with_digit() {
        let dir = tempfile::tempdir().unwrap();
        let result = create_project("1app", ProjectKind::Bin, VcsMode::Git, dir.path());
        assert!(matches!(result, Err(NewError::InvalidName(_))));
    }

    #[test]
    fn invalid_name_hyphen() {
        let dir = tempfile::tempdir().unwrap();
        let result = create_project("my-app", ProjectKind::Bin, VcsMode::Git, dir.path());
        assert!(matches!(result, Err(NewError::InvalidName(_))));
    }

    #[test]
    fn name_with_underscore_is_valid() {
        let dir = tempfile::tempdir().unwrap();
        create_project("my_app", ProjectKind::Bin, VcsMode::Git, dir.path()).unwrap();
        assert!(dir.path().join("my_app/Tyra.toml").is_file());
    }

    #[test]
    fn lib_root_module_named_after_package() {
        let (_dir, proj) = scaffold("mylib", ProjectKind::Lib);
        // Per ADR 0009: root module filename = package name
        assert!(proj.join("src/mylib.tyra").is_file());
        assert!(!proj.join("src/lib.tyra").exists());
    }

    #[test]
    fn invalid_name_uppercase() {
        let dir = tempfile::tempdir().unwrap();
        let result = create_project("MyApp", ProjectKind::Bin, VcsMode::Git, dir.path());
        assert!(matches!(result, Err(NewError::InvalidName(_))));
    }

    #[test]
    fn invalid_name_mixed_case() {
        let dir = tempfile::tempdir().unwrap();
        let result = create_project("myApp", ProjectKind::Bin, VcsMode::Git, dir.path());
        assert!(matches!(result, Err(NewError::InvalidName(_))));
    }

    #[test]
    fn invalid_name_reserved_word_fn() {
        let dir = tempfile::tempdir().unwrap();
        let result = create_project("fn", ProjectKind::Bin, VcsMode::Git, dir.path());
        assert!(matches!(result, Err(NewError::InvalidName(_))));
    }

    #[test]
    fn invalid_name_reserved_word_match() {
        let dir = tempfile::tempdir().unwrap();
        let result = create_project("match", ProjectKind::Bin, VcsMode::Git, dir.path());
        assert!(matches!(result, Err(NewError::InvalidName(_))));
    }

    #[test]
    fn invalid_name_reserved_word_import() {
        let dir = tempfile::tempdir().unwrap();
        let result = create_project("import", ProjectKind::Bin, VcsMode::Git, dir.path());
        assert!(matches!(result, Err(NewError::InvalidName(_))));
    }

    #[test]
    fn invalid_name_reserved_word_end() {
        let dir = tempfile::tempdir().unwrap();
        let result = create_project("end", ProjectKind::Bin, VcsMode::Git, dir.path());
        assert!(matches!(result, Err(NewError::InvalidName(_))));
    }

    #[test]
    fn vcs_none_skips_gitignore() {
        let dir = tempfile::tempdir().unwrap();
        create_project("myapp", ProjectKind::Bin, VcsMode::None, dir.path()).unwrap();
        let proj = dir.path().join("myapp");
        assert!(
            !proj.join(".gitignore").exists(),
            ".gitignore must not be created"
        );
        assert!(proj.join("Tyra.toml").is_file());
        assert!(proj.join("README.md").is_file());
    }

    #[test]
    fn vcs_git_creates_gitignore() {
        let dir = tempfile::tempdir().unwrap();
        create_project("myapp", ProjectKind::Bin, VcsMode::Git, dir.path()).unwrap();
        assert!(dir.path().join("myapp").join(".gitignore").is_file());
    }
}
