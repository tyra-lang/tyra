# Installation

Tyra is currently distributed as source only. You will build the compiler and runtime from source using the Rust toolchain.

## Prerequisites

- **Rust** 1.88 or later — install via [rustup.rs](https://rustup.rs)
- **Cargo** (included with Rust)
- **LLVM** 21 — required by the compiler backend (see note below)
- **Git**

> **NOTE:** On macOS, LLVM can be installed with `brew install llvm@21`. On Debian/Ubuntu, use `apt install llvm-21 clang-21`. Make sure the LLVM binaries are on your `PATH`.

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
tyra 0.1.0
implementing language spec 0.1
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

## Supported Platforms

| Platform | Status |
| --- | --- |
| macOS arm64 (Apple Silicon) | ✅ Tested |
| Linux x86_64 | ✅ Tested |
| Windows | ⚠️ Untested — build via WSL2 recommended |

## Editor Support

A VS Code extension with syntax highlighting, diagnostics, hover, go-to-definition, and completion is available as a development install.

### Step 1: Build and install the language server

```bash
cargo install --path tools/lsp/tyra-lsp
```

This installs `tyra-lsp` to `~/.cargo/bin/`. Make sure `~/.cargo/bin` is on your `PATH`.

### Step 2: Install the VS Code extension

```bash
cd tools/lsp/vscode-tyra
npm install
```

Then open the `tools/lsp/vscode-tyra/` directory in VS Code and press **F5** to launch the Extension Development Host.

> **Note:** If F5 triggers macOS voice input instead, go to System Settings → Keyboard → Dictation and change or disable the shortcut. Alternatively, use **Run → Start Debugging** from the VS Code menu, or press `Cmd+Shift+P` and run `Debug: Start Debugging`.

Alternatively, set `TYRA_LSP_PATH` to the full path of the `tyra-lsp` binary before starting VS Code:

```bash
export TYRA_LSP_PATH="$HOME/.cargo/bin/tyra-lsp"
```

> **Note:** VS Code Marketplace publication is planned for a future release.

## Next Steps

Continue to [Hello, World](02-hello-world.md) to learn the basics of the language.
