# Installation

Tyra is currently distributed as source only. You will build the compiler and runtime from source using the Rust toolchain.

## Prerequisites

- **Rust** 1.75 or later — install via [rustup.rs](https://rustup.rs)
- **Cargo** (included with Rust)
- **LLVM** 17 — required by the compiler backend (see note below)
- **Git**

> **NOTE:** On macOS, LLVM can be installed with `brew install llvm@17`. On Debian/Ubuntu, use `apt install llvm-17 clang-17`. Make sure the LLVM binaries are on your `PATH`.

## Build from Source

```bash
git clone https://github.com/tyra-lang/tyra
cd tyra
cargo build --release
```

The build takes a few minutes on first run. The resulting binary is at `target/release/tyra`.

## Environment Setup

Tyra needs to know where the standard library lives. Set the `TYRA_STDLIB` environment variable to the `stdlib/` directory in the cloned repository:

```bash
export TYRA_STDLIB="$(pwd)/stdlib"
export PATH="$PATH:$(pwd)/target/release"
```

Add these lines to your shell profile (`~/.bashrc`, `~/.zshrc`, etc.) to make them permanent:

```bash
# ~/.zshrc (or ~/.bashrc)
export TYRA_STDLIB="/path/to/tyra/stdlib"
export PATH="$PATH:/path/to/tyra/target/release"
```

> **NOTE:** Replace `/path/to/tyra` with the absolute path to your cloned repository.

## Verify the Installation

```bash
tyra --version
```

You should see output like:

```
tyra 0.1.0-dev
```

## Test with Hello, World

Create a file named `hello.tyra`:

```tyra
print("Hello, World!\n")
```

Run it:

```bash
tyra run hello.tyra
```

Expected output:

```
Hello, World!
```

If you see the output, your installation is working correctly.

## Editor Support

A **VS Code extension** with syntax highlighting and basic language support is planned for Marketplace publication. In the meantime, you can associate `.tyra` files with Ruby syntax highlighting as a rough approximation (both use `end` blocks and `#` comments).

## Next Steps

Continue to [Hello, World](02-hello-world.md) to learn the basics of the language.
