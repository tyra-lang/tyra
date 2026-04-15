# Compiler-specific guidance

This is a Rust workspace. See ../CLAUDE.md for the project-wide rules.

## Crate dependencies (do not violate this order)

tyra-diagnostics → tyra-lexer → tyra-ast → tyra-parser → tyra-resolve → tyra-types → tyra-mir → tyra-codegen-llvm → tyra-driver → tyra-cli

Each crate may depend only on those to its left. `tyra-diagnostics` is foundational and may be depended on by any crate.

## Diagnostics

All user-facing errors must use `tyra-diagnostics`. Never `eprintln!` directly.

Error format follows the spec: `error[E0042]: message at file:line:col`.
