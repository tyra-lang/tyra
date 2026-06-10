# 9. Project Lifecycle

This guide shows how to create a Tyra project from scratch, add dependencies,
and build a distributable binary — the full lifecycle introduced in v0.3.0.

---

## Creating a new project

Use `tyra new` to scaffold a project directory:

```bash
tyra new greeter
```

This creates:

```
greeter/
  Tyra.toml        # project manifest
  src/
    greeter.ty   # entry point
  .gitignore
  README.md
```

The manifest (`Tyra.toml`) looks like this:

```toml
[package]
name    = "greeter"
version = "0.1.0"
edition = "2026"
```

The generated entry point (`src/greeter.ty`):

```tyra
fn main() -> Unit
  print("Hello, Tyra!\n")
end
```

For a library project, add `--lib`:

```bash
tyra new mylib --lib
```

A library's entry point uses `export fn` instead of `fn main`, making its
declarations importable from other packages.

If you don't want a `.gitignore` generated (e.g., the project lives inside an
existing repo), pass `--vcs none`:

```bash
tyra new greeter --vcs none
```

---

## Running the project

From inside the project directory you can run without specifying a file:

```bash
cd greeter
tyra run
```

Tyra looks for `Tyra.toml` in the current directory (walking up if needed),
reads the package name, and runs `src/greeter.ty` automatically.

You can also pass the source file explicitly — this works outside a project
directory too:

```bash
tyra run src/greeter.ty
```

---

## Adding a dependency

Say you have a library at `../greet_lib` that you want to use:

```bash
tyra mod add greet_lib --path ../greet_lib
```

This appends a `[dependencies]` entry to `Tyra.toml`:

```toml
[dependencies]
greet_lib = { path = "../greet_lib" }
```

For a dependency hosted on git:

```bash
tyra mod add utils --git https://github.com/example/utils --rev abc1234
```

> **Note:** Always pin to a specific commit (`--rev`). Tyra does not yet have a
> SemVer resolver; `rev` is the reproducibility guarantee.

After adding a git dependency, fetch it:

```bash
tyra mod sync
```

Path dependencies are available immediately (no sync needed).

---

## Using a dependency

Once added, import the package by its name:

```tyra
import greet_lib

fn main() -> Unit
  let msg = greet_lib.greet(name: "World")
  print("#{msg}\n")
end
```

The import resolver checks three layers in order and errors on ambiguity
(see [ADR-0010](../design/0010-dependency-resolution.md)):

1. Local project (`src/`)
2. Declared dependencies (`[dependencies]`)
3. Standard library

---

## Inspecting the dependency tree

```bash
tyra mod tree
```

Example output:

```
greeter 0.1.0
└── greet_lib 0.1.0 (path: ../greet_lib)
    └── utils 0.1.0 (path: ../utils)
```

For machine-readable output (useful in CI or tooling):

```bash
tyra mod tree --json
```

To validate that all dependencies are consistent without mutating anything:

```bash
tyra mod sync --check
```

`sync` and `sync --check` also accept flags for CI use:

```bash
tyra mod sync --quiet           # run sync, suppress output; rely on exit code only
tyra mod sync --json            # emit a JSON report: {"synced":[], "cached":[], "skipped":[]}
tyra mod sync --check --json    # validate and emit {"ok": true, "issues": []}
```

To inspect a single dependency's resolved details:

```bash
tyra mod show greet_lib         # human-readable: source, version, cache path, synced status
tyra mod show greet_lib --json  # same info as a JSON object
```

---

## Updating a dependency

To change the source or pinned revision of an existing dependency in-place:

```bash
# Switch to a different local path
tyra mod update greet_lib --path ../greet_lib_v2

# Update the pinned git revision
tyra mod update utils --git https://github.com/example/utils --rev def5678
```

This replaces the entry in `[dependencies]` while preserving the surrounding
manifest content and the order of other entries. After updating a git dep, run
`tyra mod sync` to fetch the new revision.

---

## Removing a dependency

```bash
tyra mod remove greet_lib
```

This removes the entry from `[dependencies]` in `Tyra.toml`.

---

## Building a binary

```bash
tyra build
```

Compiles `src/greeter.ty` and writes the binary to `greeter` in the project
root. The output name matches the package name.

For a release build with optimizations:

```bash
tyra build --release
```

To specify an output path explicitly:

```bash
tyra build --release -o dist/greeter
```

---

## Cleaning the dependency cache

Fetched git dependencies are cached in `~/.tyra/cache/`. To remove the cache:

```bash
tyra mod clean
```

This does not affect path dependencies or your source files.

---

## Type-checking without running

```bash
tyra check
```

Exits 0 if the project type-checks cleanly, 1 otherwise. Useful in CI before
a full build.

---

## Converting an existing directory

If you have existing `.ty` files and want to add a manifest:

```bash
tyra mod init
```

This creates `Tyra.toml` in the current directory using the directory name as
the package name. Pass `--name` to override:

```bash
tyra mod init --name my_package
```

---

## Summary

| Command | What it does |
|---|---|
| `tyra new <name> [--lib] [--vcs none]` | Create a new project |
| `tyra run [--release]` | Run the project entry point |
| `tyra build [--release] [-o <out>]` | Compile to a native binary |
| `tyra check` | Type-check without compiling |
| `tyra mod init [--name <n>]` | Add a manifest to an existing directory |
| `tyra mod add <name> --path <p>` | Add a path dependency |
| `tyra mod add <name> --git <url> --rev <sha>` | Add a git dependency |
| `tyra mod update <name> --path <p>` | Update an existing path dependency |
| `tyra mod update <name> --git <url> --rev <sha>` | Update an existing git dependency |
| `tyra mod remove <name>` | Remove a dependency |
| `tyra mod show <name> [--json]` | Show details of one dependency |
| `tyra mod sync [--check] [--json] [--quiet]` | Fetch git deps; `--check` validates only |
| `tyra mod tree [--json]` | Show the dependency tree |
| `tyra mod clean` | Remove the local git dep cache |

---

## Next steps

- [Language Specification](../spec/ja/language-spec.md) — the authoritative
  reference for import rules (§13) and the toolchain (§18)
- [ADR-0009](../design/0009-project-manifest.md) — project manifest design
- [ADR-0010](../design/0010-dependency-resolution.md) — import resolution order
