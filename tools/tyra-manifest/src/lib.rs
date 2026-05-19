mod detect;

use serde::Deserialize;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

pub use detect::is_bin_source;

// ---------------------------------------------------------------------------
// Schema
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Manifest {
    pub package: Package,
    #[serde(default)]
    pub dependencies: HashMap<String, Dependency>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Package {
    pub name: String,
    pub version: String,
    pub edition: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Dependency {
    /// Local path to another project root (relative to the manifest's directory).
    pub path: Option<String>,
    /// HTTPS git URL.
    pub git: Option<String>,
    /// Commit SHA or tag. Required when `git` is set.
    pub rev: Option<String>,
}

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

#[derive(Debug)]
pub enum ManifestError {
    Io(std::io::Error),
    Parse(String),
    InvalidEdition(String),
    MissingRev { name: String },
    ConflictingSource { name: String },
    MissingSource { name: String },
}

impl std::fmt::Display for ManifestError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ManifestError::Io(e) => write!(f, "I/O error: {e}"),
            ManifestError::Parse(e) => write!(f, "manifest parse error: {e}"),
            ManifestError::InvalidEdition(e) => {
                write!(f, "unsupported edition \"{e}\"; only \"2026\" is valid")
            }
            ManifestError::MissingRev { name } => {
                write!(f, "dependency `{name}`: `rev` is required for git dependencies")
            }
            ManifestError::ConflictingSource { name } => {
                write!(
                    f,
                    "dependency `{name}`: specify exactly one of `path` or `git`, not both"
                )
            }
            ManifestError::MissingSource { name } => {
                write!(f, "dependency `{name}`: must specify `path` or `git`")
            }
        }
    }
}

impl std::error::Error for ManifestError {}

impl From<std::io::Error> for ManifestError {
    fn from(e: std::io::Error) -> Self {
        ManifestError::Io(e)
    }
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Walk up from `start` looking for a `Tyra.toml`. Returns the directory that
/// contains the manifest, or `None` if no manifest is found.
pub fn find_project_root(start: &Path) -> Option<PathBuf> {
    let mut dir = if start.is_file() {
        start.parent()?.to_path_buf()
    } else {
        start.to_path_buf()
    };
    loop {
        if dir.join("Tyra.toml").is_file() {
            return Some(dir);
        }
        if !dir.pop() {
            return None;
        }
    }
}

/// Read and validate `Tyra.toml` in `project_root`.
pub fn load_manifest(project_root: &Path) -> Result<Manifest, ManifestError> {
    let path = project_root.join("Tyra.toml");
    let text = std::fs::read_to_string(&path)?;
    let manifest: Manifest =
        toml::from_str(&text).map_err(|e| ManifestError::Parse(e.to_string()))?;
    validate(&manifest)?;
    Ok(manifest)
}

// ---------------------------------------------------------------------------
// Validation
// ---------------------------------------------------------------------------

fn validate(manifest: &Manifest) -> Result<(), ManifestError> {
    if manifest.package.edition != "2026" {
        return Err(ManifestError::InvalidEdition(
            manifest.package.edition.clone(),
        ));
    }
    for (name, dep) in &manifest.dependencies {
        match (&dep.path, &dep.git) {
            (Some(_), Some(_)) => {
                return Err(ManifestError::ConflictingSource { name: name.clone() })
            }
            (None, None) => return Err(ManifestError::MissingSource { name: name.clone() }),
            (None, Some(_)) if dep.rev.is_none() => {
                return Err(ManifestError::MissingRev { name: name.clone() })
            }
            _ => {}
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(s: &str) -> Result<Manifest, ManifestError> {
        toml::from_str::<Manifest>(s)
            .map_err(|e| ManifestError::Parse(e.to_string()))
            .and_then(|m| {
                validate(&m)?;
                Ok(m)
            })
    }

    #[test]
    fn minimal_bin_manifest() {
        let m = parse(
            r#"
[package]
name    = "myapp"
version = "0.1.0"
edition = "2026"
"#,
        )
        .unwrap();
        assert_eq!(m.package.name, "myapp");
        assert!(m.dependencies.is_empty());
    }

    #[test]
    fn path_dependency() {
        let m = parse(
            r#"
[package]
name    = "myapp"
version = "0.1.0"
edition = "2026"

[dependencies]
mylib = { path = "../mylib" }
"#,
        )
        .unwrap();
        assert_eq!(m.dependencies["mylib"].path.as_deref(), Some("../mylib"));
    }

    #[test]
    fn git_dependency() {
        let m = parse(
            r#"
[package]
name    = "myapp"
version = "0.1.0"
edition = "2026"

[dependencies]
utils = { git = "https://github.com/example/utils.git", rev = "abc1234" }
"#,
        )
        .unwrap();
        assert_eq!(m.dependencies["utils"].rev.as_deref(), Some("abc1234"));
    }

    #[test]
    fn git_dependency_missing_rev_is_error() {
        let result = parse(
            r#"
[package]
name    = "myapp"
version = "0.1.0"
edition = "2026"

[dependencies]
utils = { git = "https://github.com/example/utils.git" }
"#,
        );
        assert!(
            matches!(result, Err(ManifestError::MissingRev { .. })),
            "expected MissingRev, got {result:?}"
        );
    }

    #[test]
    fn conflicting_path_and_git_is_error() {
        let result = parse(
            r#"
[package]
name    = "myapp"
version = "0.1.0"
edition = "2026"

[dependencies]
bad = { path = "../bad", git = "https://example.com/bad.git", rev = "abc" }
"#,
        );
        assert!(
            matches!(result, Err(ManifestError::ConflictingSource { .. })),
            "expected ConflictingSource, got {result:?}"
        );
    }

    #[test]
    fn invalid_edition_is_error() {
        let result = parse(
            r#"
[package]
name    = "myapp"
version = "0.1.0"
edition = "2025"
"#,
        );
        assert!(
            matches!(result, Err(ManifestError::InvalidEdition(_))),
            "expected InvalidEdition, got {result:?}"
        );
    }

    #[test]
    fn unknown_top_level_key_is_error() {
        let result = toml::from_str::<Manifest>(
            r#"
[package]
name    = "myapp"
version = "0.1.0"
edition = "2026"

[unknown_section]
foo = "bar"
"#,
        );
        assert!(result.is_err(), "unknown key must be rejected");
    }

    #[test]
    fn find_project_root_finds_manifest() {
        let dir = tempfile::tempdir().unwrap();
        let src_dir = dir.path().join("src");
        std::fs::create_dir(&src_dir).unwrap();
        std::fs::write(
            dir.path().join("Tyra.toml"),
            "[package]\nname=\"x\"\nversion=\"0.1.0\"\nedition=\"2026\"\n",
        )
        .unwrap();
        let found = find_project_root(&src_dir).unwrap();
        assert_eq!(found, dir.path());
    }

    #[test]
    fn find_project_root_returns_none_without_manifest() {
        let dir = tempfile::tempdir().unwrap();
        assert!(find_project_root(dir.path()).is_none());
    }
}
