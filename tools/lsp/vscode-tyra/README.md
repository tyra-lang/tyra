# vscode-tyra

VS Code extension for the [Tyra](https://github.com/kiyoshi/tyra-lang) programming language.

## Features

- Syntax highlighting for `.tyra` files
- Diagnostics (compiler errors shown as red squigglies on save)
- Hover: shows the inferred type of identifiers

## Installation (development)

1. Build and install the language server:

```sh
cargo install --path tools/lsp/tyra-lsp
```

2. Open this directory in VS Code and press **F5** to launch the Extension Development Host.

Alternatively, set `TYRA_LSP_PATH` to the path of the `tyra-lsp` binary before starting VS Code.

## Requirements

- `tyra-lsp` binary on `$PATH` (or `TYRA_LSP_PATH` environment variable)
- VS Code 1.85 or later
