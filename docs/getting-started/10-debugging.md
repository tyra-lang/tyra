# 10. Debugging

Tyra v0.6.0 ships a DAP-based debugger backed by `lldb-dap`. Non-release builds
include DWARF debug information, so you can set breakpoints, step through code,
and inspect local variables directly in VS Code.

---

## Prerequisites

You need `lldb-dap` installed and visible to the VS Code extension. The
extension searches the following locations in order:

1. The path in the `LLDB_DAP_PATH` environment variable (if set)
2. `/Applications/Xcode.app/Contents/Developer/usr/bin/lldb-dap` (macOS + Xcode)
3. `/opt/homebrew/opt/llvm/bin/lldb-dap`
4. `/opt/homebrew/opt/llvm@19/bin/lldb-dap`
5. `/usr/local/opt/llvm/bin/lldb-dap`
6. `/usr/bin/lldb-dap`

### macOS

**Option A — Xcode** (already installed for most developers):

```bash
xcode-select --install   # if Xcode Command Line Tools are not yet installed
```

`lldb-dap` will be at the default Xcode path (location 2 above) and picked up
automatically.

**Option B — Homebrew LLVM**:

```bash
brew install llvm
```

This installs `lldb-dap` at `$(brew --prefix llvm)/bin/lldb-dap` (typically
`/opt/homebrew/opt/llvm/bin/lldb-dap` on Apple Silicon or
`/usr/local/opt/llvm/bin/lldb-dap` on Intel), both of which the extension
checks automatically.

### Linux

Install the LLVM package for your distro:

```bash
# Debian / Ubuntu
sudo apt install llvm lldb

# Fedora / RHEL
sudo dnf install llvm lldb
```

The installed path varies by distro and LLVM version. If the extension cannot
find `lldb-dap` automatically, set the environment variable explicitly:

```bash
export LLDB_DAP_PATH=/usr/lib/llvm-19/bin/lldb-dap   # adjust version
```

Add this to your shell profile so VS Code inherits it.

### Windows

Not supported in v0.6.0.

---

## VS Code setup

Install the **vscode-tyra** extension. During development, use a local
extension install:

```bash
cd tools/lsp/vscode-tyra
npm install
npm run package        # produces vscode-tyra-*.vsix
code --install-extension vscode-tyra-*.vsix
```

Then create `.vscode/launch.json` in your project root:

```json
{
  "version": "0.2.0",
  "configurations": [
    {
      "type": "tyra",
      "request": "launch",
      "name": "Debug Tyra program",
      "program": "${workspaceFolder}/my_program",
      "args": [],
      "cwd": "${workspaceFolder}"
    }
  ]
}
```

Set `"program"` to the path of the compiled binary (see the next section).

---

## Building a debug binary

Build without `--release` to include DWARF debug information:

```bash
tyra build src/main.ty -o my_program
```

Or, inside a project directory:

```bash
tyra build
```

The resulting binary contains full DWARF info. Release builds omit it:

```bash
tyra build --release   # DWARF stripped — not suitable for debugging
```

---

## Setting breakpoints and stepping

1. Open your `.ty` source file in VS Code.
2. Click in the gutter to set a breakpoint (a red dot appears).
3. Press **F5** (or run **Debug > Start Debugging**) to launch the session.

The debugger stops at the breakpoint. Use the standard VS Code controls to
step over (**F10**), step into (**F11**), or continue (**F5**).

---

## Viewing locals

The **Variables** pane shows local variables while the debugger is paused.

| Type | Behavior |
|---|---|
| `Int`, `Bool` | Shows the numeric / boolean value |
| `String` | Shows the string contents |
| Closures | Shows as a struct with two integer fields (`fn_ptr`, `env_ptr`) |
| Recursive ADTs | Shows as a pointer; dereference manually (see below) |

For a recursive ADT value (e.g., a linked list node stored on the GC heap),
LLDB displays a raw pointer. To inspect the pointed-to value, use the **Debug
Console** (bottom of VS Code) and enter an LLDB expression:

```
expr *(MyNode *)0x<address>
```

Replace `0x<address>` with the pointer value shown in the Variables pane.

---

## Known limitations

- **Optimized builds** — locals are only accurate in debug builds (`tyra build`
  without `--release`). With `-O1` or higher the optimizer may eliminate or
  combine variables, making the Variables pane unreliable.
- **Closures** — represented as a fat pointer (`{ fn_ptr, env_ptr }`); the
  Variables pane shows two integer fields rather than the closure's captured
  variables.
- **Recursive ADTs** — GC-boxed values appear as opaque pointers. Manual
  dereference via LLDB expressions is required to inspect fields.
- **Linux** — `lldb-dap` path varies by distribution and LLVM version. If
  auto-detection fails, set `LLDB_DAP_PATH` explicitly.
- **Windows** — not supported.

---

## Troubleshooting

### "Tyra debugger: lldb-dap not found"

The extension could not locate `lldb-dap` in any of the default paths.

1. Verify the binary exists:

   ```bash
   which lldb-dap          # or: ls $(brew --prefix llvm)/bin/lldb-dap
   ```

2. If found, set the environment variable so the extension picks it up:

   ```bash
   export LLDB_DAP_PATH=/path/to/lldb-dap
   ```

   Then restart VS Code (it must inherit the updated environment).

3. If not found, install `lldb-dap` following the [Prerequisites](#prerequisites)
   section above.

### Breakpoints not hit

- Confirm the binary was built **without** `--release`.
- Confirm `"program"` in `launch.json` points to the correct binary.
- Recompile after source changes — the binary must match the source on disk.

---

## Summary

| Step | Command / action |
|---|---|
| Install lldb-dap (macOS/Xcode) | `xcode-select --install` |
| Install lldb-dap (Homebrew) | `brew install llvm` |
| Override lldb-dap path | `export LLDB_DAP_PATH=/path/to/lldb-dap` |
| Build a debug binary | `tyra build` (no `--release`) |
| Start debugging | F5 in VS Code with a `launch.json` configured |

---

## Next steps

- [9. Project Lifecycle](09-project-lifecycle.md) — building and managing projects
- [Language Specification](../spec/ja/language-spec.md) — authoritative language reference
