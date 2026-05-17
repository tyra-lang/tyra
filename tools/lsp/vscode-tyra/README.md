# vscode-tyra

VS Code extension for the [Tyra](https://github.com/tyra-lang/tyra) programming language.

## Features

- Syntax highlighting for `.tyra` files
- Diagnostics (compiler errors shown as red squigglies on save)
- Hover: shows the inferred type of identifiers

## Installation (development)

1. Build and install the language server:

```sh
cargo install --path tools/lsp/tyra-lsp
```

2. Install npm dependencies:

```sh
npm install
```

3. Open this directory in VS Code and press **F5** to launch the Extension Development Host.

> **Note (macOS):** If F5 triggers voice input, disable the shortcut in System Settings → Keyboard → Dictation. Alternatively, use **Run → Start Debugging** or `Cmd+Shift+P` → `Debug: Start Debugging`.

Alternatively, set `TYRA_LSP_PATH` to the path of the `tyra-lsp` binary before starting VS Code.

## Requirements

- `tyra-lsp` binary on `$PATH` (or `TYRA_LSP_PATH` environment variable)
- VS Code 1.85 or later
