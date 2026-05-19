# ADR 0009: Project Manifest and Package Namespace (v0.3)

- **Status**: Accepted
- **Date**: 2026-05-19
- **Spec sections affected**: §13.1, §13.2, §18

---

## Context

Tyra v0.2 has no concept of a "project". Every invocation of `tyra run`,
`tyra build`, or `tyra check` receives a single file path. The only automatic
module resolution is:

1. The directory of the source file (`main_dir`) — for local modules
2. A `stdlib/` directory found by walking up from `main_dir` — for the standard library

This is sufficient for single-file programs and small experiments but prevents:

- Multiple source files organized across directories from being compiled as a
  unit without manually managing paths
- Declaring external dependencies (code from other repositories)
- Tooling (`tyra new`, `tyra mod`) from having a stable place to read and write
  project metadata

The goal is to introduce the minimal manifest needed to unlock these use cases
without imposing cargo-level complexity on users who write single-file scripts.

---

## Decision

### The manifest: `Tyra.toml`

A file named `Tyra.toml` in a directory marks that directory as a **project
root**. The toolchain walks up from the source file to find it (the same
algorithm used by `find_stdlib_dir` in `tyra-driver/src/lib.rs:228`).

Single-file programs without a `Tyra.toml` are still valid. The manifest is
opt-in.

#### Minimum schema

```toml
[package]
name    = "myapp"
version = "0.1.0"
edition = "2026"

[dependencies]
some_lib  = { path = "../some_lib" }
other_lib = { git = "https://github.com/user/other.git", rev = "abc123" }
```

Fields:

| Field | Required | Type | Notes |
|---|---|---|---|
| `package.name` | yes | string | Package name; also the root module name for lib projects |
| `package.version` | yes | string | Semver string, informational only in v0.3 |
| `package.edition` | yes | string | `"2026"` is the only valid value in v0.3 |
| `dependencies.<name>.path` | — | string | Relative path to another project root |
| `dependencies.<name>.git` | — | string | HTTPS git URL |
| `dependencies.<name>.rev` | — | string | Commit SHA or tag; required when `git` is used |

Unknown top-level keys are rejected (forward-compat guard). Unknown keys inside
`[dependencies]` entries are rejected for the same reason.

`[bin]` and `[lib]` explicit table entries are **not** introduced in v0.3. The
project type is inferred from source content (see below).

#### `edition`

The `edition` field is a forward-compatibility guard. Its value today is always
`"2026"`. Future editions may change language or module semantics; the compiler
will reject manifests whose edition is higher than the compiler's own edition.
Using the calendar year (Rust convention) avoids coupling the edition to a
release number, which makes the value stable across patch releases.

---

### Source layout

A project root with manifest `Tyra.toml` and `[package].name = "myapp"` is
expected to have its source under `src/`:

```
myapp/
  Tyra.toml
  src/
    myapp.tyra          ← root module (filename = package name)
    myapp/
      helper.tyra       ← submodule
      io.tyra           ← submodule
```

Rules:

1. **Root module filename equals package name.** The root module is
   `src/<name>.tyra`. This is a direct application of spec §13.1 ("file name
   matches module name") to the package level.

2. **Submodules live in `src/<name>/<sub>.tyra`.** They are imported as
   `import myapp.helper`, `import myapp.io`, following the existing dot-path
   convention (§13.2).

3. **`src/lib.tyra` is not a valid root module name** for a package named
   anything other than `lib`. A package named `lib` itself is discouraged (it
   would produce `import lib`, which reads as a reference to a vague concept
   rather than a concrete package).

---

### Bin / lib distinction (implicit, content-based)

There is no `[bin]` / `[lib]` table in v0.3. Instead, the compiler inspects the
root module (`src/<name>.tyra`) to determine the project type:

| Content of `src/<name>.tyra` | Project type |
|---|---|
| Contains `fn main` or top-level executable statements (ADR 0006) | **bin** |
| Contains declarations only (`fn`, `type`, `value`, `data`, `trait`, `impl`, `import`, `export`) | **lib** |

This rule is consistent with spec §13.1 (module files may only contain
declarations) and ADR 0006 (top-level executable statements are an entry-point
feature).

#### Bin packages cannot be imported as dependencies

A bin package — one whose root module contains `fn main` or top-level
executable statements — **cannot be declared as a `[dependencies]` entry** in
another project.

Enforcement points:

- `tyra mod sync` performs a lightweight parse of the dependency's root module
  to detect `fn main` or top-level executable statements before caching. If
  found, it fails with `E_DEP_NOT_IMPORTABLE`.
- `resolve_imports` (in `tyra-driver`) checks the same condition when resolving
  an `import` statement that points into a dependency's source tree. If the
  dependency's root is a bin, it fails with `E_DEP_NOT_IMPORTABLE`.

The two-layer check (manifest-time and compile-time) prevents the error from
being silently deferred to link time.

---

### Project root discovery

The toolchain locates the project root by walking up from the file being
compiled:

```
file.tyra → parent dir → parent dir → ... → dir containing Tyra.toml (or filesystem root)
```

This reuses the algorithm in `find_stdlib_dir` (`tyra-driver/src/lib.rs:228`).
A new function `find_project_root(start_dir) -> Option<PathBuf>` will be added
to the `tyra-manifest` crate.

If no `Tyra.toml` is found, the file is treated as a standalone script (v0.1 /
v0.2 behaviour, preserved for backward compatibility).

---

## Alternatives considered

### A. `[bin]` / `[lib]` explicit tables (Cargo-style)

```toml
[bin]
name = "myapp"
path = "src/main.tyra"

[lib]
name = "myapp"
path = "src/lib.tyra"
```

**Rejected for v0.3.** This requires the user to declare information that is
already unambiguously encoded in the source: whether a file contains `fn main`
or not. The implicit rule is shorter to write and impossible to get out of sync
with the actual code. Explicit tables remain an option for a future ADR if
there is demand for non-default layouts.

### B. Root module named `src/main.tyra` for bin, `src/lib.tyra` for lib

A fixed-name convention (Go-style `main.go`, Rust's `src/main.rs` /
`src/lib.rs`).

**Rejected.** Fixed names violate spec §13.1 ("file name matches module name"):
`import src.lib` or `import main` are not what users expect. The `src/lib.tyra`
name in particular would make `import myapp.lib` the correct import path, which
is confusing. Using `src/<package-name>.tyra` as the root is a natural
application of the existing module naming rule.

### C. Single-file projects with inline `[dependencies]`

A comment-based or special-section approach where the manifest lives inside the
`.tyra` file itself (similar to Deno's `// @deno-types` or Python's inline
script metadata).

**Rejected.** Mixes metadata and code, complicates the parser, and cannot be
found without parsing the file first. `Tyra.toml` is a conventional, tooling-
friendly location.

### D. `edition = "0.3"` (version-tied edition)

Use the Tyra release version as the edition value.

**Rejected.** Coupling the edition to the release number means that a `0.3.1`
patch release technically introduces a new edition string even if nothing
language-observable changed. The calendar year (Rust convention) is stable
across patch releases.

---

## Consequences

- `tools/tyra-manifest/` is a new crate that owns the `Tyra.toml` parser and
  `find_project_root`. It depends on `toml` (serde-derive) only.
- `tyra-driver::resolve_imports` is extended to consult `[dependencies]`
  resolved by `tyra-manifest`. This is the minimal change to the existing
  resolver (see ADR 0010 for the full resolution algorithm).
- Existing single-file programs and the static conformance corpus continue to
  work without any `Tyra.toml` (backward-compatible path).
- `tyra new` (v0.3) generates a project layout conforming to this ADR.
- `tyra mod sync` enforces `E_DEP_NOT_IMPORTABLE` at dependency-fetch time.
- The spec §13.1 note "v0.1 では module-level initialization semantics を定義し
  ない" continues to hold. `Tyra.toml` introduces project structure without
  changing module semantics.
