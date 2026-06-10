# ADR-0025: Source file extension `.tyra` → `.ty`

**Status**: Accepted  
**Date**: 2026-06-10  
**Deciders**: Kiyoshi Mizumoto (sole maintainer)

## Context

The Tyra source file extension `.tyra` (4 characters) was chosen at project inception for uniqueness. As the project matured, the length became a daily friction point: tab-completion, shell globs, and file names in error messages are all 2 characters longer than necessary.

## Decision

Rename the source file extension from `.tyra` to `.ty`, effective v0.10.0.

### Scope of change

- **Changed**: all `*.tyra` source files renamed to `*.ty`; all compiler/tooling/LSP/CI extension checks updated to `"ty"`
- **Unchanged** (not the file extension):
  - Language ID `"tyra"` (LSP, VS Code `language` field)
  - TextMate grammar scope names (`source.tyra`, `comment.line.double-slash.tyra`, …)
  - LLVM codegen symbols (`@.tyra.argc`, `@.tyra.argv`, `.tyra_counters`)
  - Cache directory `~/.tyra/` and install layout `lib/tyra/`

### Alternatives considered

| Option | Rejected because |
|---|---|
| Keep `.tyra` | Ongoing ergonomic friction; no benefit to 4-char extension |
| Both `.ty` and `.tyra` (dual-accept) | Permanent resolver complexity; `*.tyra` globs in tests/CI would diverge |
| Use `.tr` | `tr` is a standard POSIX utility; cognitive collision |
| Use `.ta` / `.yr` | Weak association with "tyra" |

## Consequences

- **Breaking change** for any existing `.tyra` source files outside this repository. Since v0.10.0 is a pre-1.0 release, this is acceptable under the project's semver policy.
- `import` statements are **not affected**: the module resolver appended `.tyra` internally; it now appends `.ty`. Source-level `import foo` syntax is unchanged.
- The `is_tyra_stdlib` sentinel (previously `assert.tyra`, now `assert.ty`) was updated atomically with the stdlib rename.
