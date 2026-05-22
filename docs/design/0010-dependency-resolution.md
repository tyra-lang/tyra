# ADR 0010: Import Resolution and Dependency Lookup (v0.3)

- **Status**: Accepted
- **Date**: 2026-05-19
- **Spec sections affected**: §13.2, §18
- **Related**: ADR 0009 (project manifest)

---

## Context

### Current resolution (v0.2)

`resolve_imports` in `tyra-driver/src/lib.rs:252` uses a two-stage precedence
model:

1. Build `<main_dir>/a/b/c.tyra` from the import path segments.
2. If the file does not exist, build `<stdlib_dir>/a/b/c.tyra` using the
   stdlib directory found by `find_stdlib_dir` (walk-up or `TYRA_STDLIB`).
3. If neither exists, emit `error[E0200]`.

This is simple but has two problems for v0.3:

- **No third slot for dependencies.** There is no place to insert
  `[dependencies]` resolution between the local and stdlib searches.
- **Precedence silently shadows.** If a local file accidentally has the same
  name as a stdlib module (`math.tyra`, `json.tyra`), the local file wins
  without any warning. Adding a new stdlib module in a future release could
  likewise silently shadow a local file in user projects.

### Goal for v0.3

Extend resolution to three layers (local, deps, stdlib) while eliminating
silent shadowing. The `[dependencies]` mechanism introduced by ADR 0009 needs
a concrete resolution algorithm.

---

## Decision

### Resolution model: exhaustive search with uniqueness rule

For each `import a.b.c` statement, the resolver **searches all layers** and
collects every candidate path that satisfies the import:

| Layer | Search path |
|---|---|
| **(a) Local** | `<project_root>/src/a/b/c.tyra` (or `<main_dir>/a/b/c.tyra` for standalone scripts without `Tyra.toml`) |
| **(b) Dependencies** | For **git** deps: `~/.tyra/cache/git/<dep-key>/src/a/b/c.tyra`. For **path** deps: `<resolved-path>/src/a/b/c.tyra` (resolved directly on disk, not cached). Searched for each `[dependencies]` entry whose package name matches `a`. |
| **(c) Stdlib** | `<stdlib_dir>/a/b/c.tyra` (unchanged from v0.2) |

Built-in modules (`core.sys`, etc.) bypass all layers and are handled by
codegen directly, as in v0.2.

**Outcome by candidate count:**

| Candidates found | Result |
|---|---|
| **0** | `error[E0200]` — module not found (existing code, message unchanged) |
| **1** | Success — that candidate is used |
| **2 or more** | `error[E0217]` `E_IMPORT_AMBIGUOUS` — name collision across layers; user must resolve |

This is a **uniqueness rule**, not a precedence rule. The resolver never
silently picks one candidate over another. If two layers both provide `foo`,
the build fails with a clear error naming both paths.

#### Rationale for uniqueness over precedence

Precedence (local > deps > stdlib) is intuitive for simple cases but produces
surprising failures at scale:

- A future stdlib release adds `math.vector`. Any user project with a local
  `math/vector.tyra` would silently have its local file used — but they would
  never know, and the stdlib `math.vector` would be inaccessible.
- A dependency ships an update that adds a module whose name collides with
  another dependency. The user's build would silently change behaviour
  depending on dependency declaration order.

Uniqueness forces the collision to be visible and actionable. The fix is always
the same: rename the local module or use an `import ... as alias` form.

---

### Layer (a): Local source

For projects with a `Tyra.toml` (project mode), the local layer is
`<project_root>/src/`. The project root is found by `find_project_root` from
ADR 0009.

For standalone scripts without `Tyra.toml` (script mode), the local layer is
`<main_dir>/` — identical to v0.2 behaviour. Script mode has no `[dependencies]`
layer.

#### Transition: `main_dir` → `project_root/src/`

In project mode, `main_dir` is no longer used as the local search root.
`tyra run src/myapp.tyra` inside a project resolves imports from
`<project_root>/src/`, not from `<main_dir>` (which is the same directory in
this example, but diverges for nested files).

The compiler determines the mode by checking whether `find_project_root`
returns `Some`. If so, project mode; otherwise, script mode. The static
conformance corpus (`bench/static-corpus/`) contains no `Tyra.toml` and
therefore always runs in script mode — backward compatibility is preserved.

---

### Layer (b): Dependencies

A dependency named `some_lib` in `[dependencies]` contributes to the resolver
only if the first path segment of the import matches the dependency name:
`import some_lib.helper` → search in `some_lib`'s cache entry.

#### Cache layout

```
~/.tyra/cache/
  git/<host>/<user>/<repo>/<rev>/   ← git dependencies
    Tyra.toml
    src/
      some_lib.tyra
      some_lib/
        helper.tyra
  path/<absolute-path-hash>/        ← path dependencies (symlink or copy)
    Tyra.toml
    src/
      ...
```

The cache key for git dependencies is `<host>/<user>/<repo>/<rev>` — the full
rev (SHA or tag) is used as-is, providing reproducibility without a lockfile.
Re-running `tyra mod sync` with the same rev is always a no-op if the cache
entry already exists.

Path dependencies are **not** cached. The resolver reads them directly from the
on-disk path declared in `Tyra.toml`. The `path/<absolute-path-hash>/` slot in
the cache layout above is reserved for future tooling that may need a stable
indirection layer; it is not used in v0.3.

#### `tyra mod sync` behaviour

`tyra mod sync`:

1. Reads `Tyra.toml` in the current project root.
2. For each `{ git = "...", rev = "..." }` dependency:
   a. Checks whether `~/.tyra/cache/git/<key>/` already exists. If so, skips.
   b. Runs `git clone <url> <tmpdir>` (full clone, no `--depth 1`) followed by
      `git -C <tmpdir> checkout <rev>` to obtain the pinned state. A full clone
      is required because `--depth 1` only fetches the default branch HEAD;
      arbitrary commit SHAs and tags that are not at HEAD are unreachable in a
      shallow clone.
   c. Performs a lightweight parse of the dependency's root module
      (`<tmpdir>/src/<dep-name>.tyra`) to verify it is a lib (declarations
      only). If the root module contains `fn main` or top-level executable
      statements, deletes `<tmpdir>` and fails with `E_DEP_NOT_IMPORTABLE`
      **without** writing anything to the cache.
   d. Moves `<tmpdir>` to the cache slot atomically. The cache slot is either
      absent (success) or fully present (previous success); a partial write
      never occurs.
3. For each `{ path = "..." }` dependency: validates the path exists and
   contains a `Tyra.toml`. Performs the same bin/lib check.
4. Prints a summary of fetched / already-present / failed entries.

**Git backend**: `Command::new("git")` (system `git`). The `git2` crate is not
used in v0.3. This is simpler to maintain and avoids a large native dependency.
Windows support (where `git` may not be in `PATH`) is deferred.

---

### No lockfile in v0.3

`Tyra.lock` is **not** introduced in v0.3. Reproducibility is ensured by
requiring `rev` (a full SHA or named tag) for every git dependency. Two
developers running `tyra mod sync` with the same `Tyra.toml` always fetch the
same commit.

Consequences:

- `rev` is **required** for `git` dependencies; omitting it is a manifest parse
  error. There is no "latest commit" shorthand.
- `path` dependencies have no pinning mechanism; they reflect the current
  on-disk state of the referenced project.

`Tyra.lock` will be introduced in **v0.4.0** together with floating version
constraints and a minimal transitive dependency solver. The `~/.tyra/cache/`
structure is designed to be lockfile-compatible (each entry is keyed by exact
rev). A full registry-backed resolver and `tyra publish` are planned for v0.5+.

---

### Error codes

| Code | Meaning |
|---|---|
| `E0200` | Import not found (existing code; unchanged) |
| `E0217` | `E_IMPORT_AMBIGUOUS` — same import path resolved in two or more layers |
| `E0218` | `DepConflict` — same package required at two incompatible revisions (or inconsistent `rev`/`branch` constraints) |
| `E0219` | `LockfileOutOfSync` — `Tyra.lock` is inconsistent with the manifest or path-dep graph (missing dep, source/rev/branch change, constraint-type change, or transitive path dep added/removed); run `tyra mod sync` |
| `E0220` | `DepNameCollision` — two distinct sources both claim the same import name; a single import name must resolve to exactly one package |
| `E_DEP_NOT_IMPORTABLE` | Dependency root is a bin package (has `fn main` or top-level executable statements) |

`E0217` message format:
```
error[E0217]: ambiguous import `some_lib`
  --> src/myapp.tyra:3:1
   |
 3 | import some_lib
   | ^^^^^^^^^^^^^^^
   = note: found in local source:      /home/user/myapp/src/some_lib.tyra
   = note: found in dependency cache:  ~/.tyra/cache/git/github.com/user/some_lib/abc123/src/some_lib.tyra
```

---

## Alternatives considered

### A. Precedence: local > deps > stdlib

The first candidate found wins. Silent shadowing is possible but the rule is
simple.

**Rejected.** See "Rationale for uniqueness over precedence" above. Silent
shadowing makes library upgrades and new stdlib additions risky. The uniqueness
rule is slightly stricter but the error is always actionable.

### B. Namespace-prefixed imports for dependencies

Dependencies are always imported with a package-qualified path:
`import deps.some_lib.helper` instead of `import some_lib.helper`.

**Rejected.** The extra `deps.` prefix is verbose and breaks the existing
`import a.b.c` convention. Users who split a project into local modules and a
dependency should not need to change import paths when they extract code.

### C. `git2` crate for the git backend

Use the `git2` Rust crate (libgit2 bindings) instead of shelling out to `git`.

**Rejected for v0.3.** `git2` adds a large C native dependency (libgit2),
complicates cross-compilation, and is unnecessary given the simple `clone +
checkout` workflow. `Command::new("git")` is sufficient. Re-evaluate if
Windows-native support or authenticated HTTPS becomes a requirement.

### D. Lockfile from the start

Introduce `Tyra.lock` in v0.3 alongside `Tyra.toml`.

**Rejected.** In v0.3 there are no floating version constraints (every `git`
dep requires an exact `rev`), so a lockfile would duplicate information already
in `Tyra.toml`. The complexity cost of implementing lockfile generation,
parsing, and conflict resolution is not justified by the benefit. Deferred to
when floating constraints are introduced.

---

## Consequences

- `tyra-driver::resolve_imports` (lib.rs:252) is rewritten to implement
  exhaustive three-layer search and uniqueness check. The existing two-stage
  precedence logic is replaced.
- `resolve_import_file` (lib.rs:390) is updated to accept a resolved cache
  directory in addition to `main_dir` and `stdlib_dir`.
- `tools/tyra-pkg/` (`tyra mod sync`) implements the git clone and cache
  management logic described above.
- The static conformance corpus (`bench/static-corpus/`) is unaffected:
  no `Tyra.toml` present → script mode → layer (a) = `main_dir`, layer (b)
  absent, layer (c) = stdlib. Identical to v0.2 behaviour.
- `E0200` ("module not found") error message is unchanged. `E0217` is new.
- `TYRA_STDLIB` continues to override the stdlib directory location. It is
  not a resolution precedence mechanism.
