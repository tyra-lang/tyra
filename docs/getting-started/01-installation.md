# Installation

Pre-built binaries are available for macOS (Apple Silicon) and Linux (x86_64, musl/Alpine).

## Quick Install (curl | sh)

```bash
curl -fsSL https://raw.githubusercontent.com/tyra-lang/tyra/main/scripts/install.sh | sh
```

This installs `tyra` to `~/.local/bin` and the runtime library + stdlib to `~/.local/lib/tyra/`.

**Options:**

```bash
# Install to a custom prefix (e.g. /usr/local)
curl -fsSL https://raw.githubusercontent.com/tyra-lang/tyra/main/scripts/install.sh | sh -s -- --prefix /usr/local

# Install a specific version
curl -fsSL https://raw.githubusercontent.com/tyra-lang/tyra/main/scripts/install.sh | sh -s -- --version v0.10.0
```

After installation, add `~/.local/bin` to your `PATH` if not already present:

```bash
export PATH="$HOME/.local/bin:$PATH"
```

Then verify:

```bash
tyra --version
```

## Homebrew (macOS)

```bash
brew install tyra-lang/tap/tyra
```

> **Note:** The Homebrew tap is published alongside the v0.10.0 release. On first use, run `brew tap tyra-lang/tap` if the above command does not resolve automatically.

## Build from Source

### Prerequisites

- **Rust** 1.88 or later — install via [rustup.rs](https://rustup.rs)
- **Cargo** (included with Rust)
- **LLVM** 22 — required by the compiler backend (see note below)
- **Git**

> **NOTE:** On macOS, install with `brew install llvm@22`. On Debian/Ubuntu, use `apt install llvm-22 clang-22`. Make sure the LLVM binaries are on your `PATH`. (LLVM 21 also works — pass `--features llvm21-1` to `cargo build`.)

### Build

```bash
git clone https://github.com/tyra-lang/tyra
cd tyra
cargo build --release
```

The build takes a few minutes on first run. The resulting binary is at `target/release/tyra`.

### Environment Setup

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

## Verify the Installation (source build)

```bash
tyra --version
```

You should see output like:

```
tyra 0.1.0
implementing language spec 0.1
```

## Test with Hello, World

Create a file named `hello.ty`:

```tyra
print("Hello, World!\n")
```

Run it:

```bash
tyra run hello.ty
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
