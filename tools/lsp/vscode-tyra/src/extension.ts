import * as fs from "fs";
import {
  DebugAdapterDescriptor,
  DebugAdapterDescriptorFactory,
  DebugAdapterExecutable,
  DebugSession,
  ExtensionContext,
  ProviderResult,
  debug,
  window,
  workspace,
} from "vscode";
import {
  LanguageClient,
  LanguageClientOptions,
  ServerOptions,
  TransportKind,
} from "vscode-languageclient/node";

let client: LanguageClient | undefined;

export function activate(context: ExtensionContext): void {
  // Resolve the tyra-lsp binary: prefer TYRA_LSP_PATH env var, then PATH.
  const serverBin = process.env["TYRA_LSP_PATH"] ?? "tyra-lsp";

  const serverOptions: ServerOptions = {
    run: { command: serverBin, transport: TransportKind.stdio },
    debug: { command: serverBin, transport: TransportKind.stdio },
  };

  const clientOptions: LanguageClientOptions = {
    documentSelector: [{ scheme: "file", language: "tyra" }],
    synchronize: {
      fileEvents: workspace.createFileSystemWatcher("**/*.ty"),
    },
  };

  client = new LanguageClient(
    "tyra-lsp",
    "Tyra Language Server",
    serverOptions,
    clientOptions
  );

  client.start();
  context.subscriptions.push(client);

  const factory = new TyraDapDescriptorFactory();
  context.subscriptions.push(
    debug.registerDebugAdapterDescriptorFactory("tyra", factory)
  );
}

export async function deactivate(): Promise<void> {
  if (client) {
    await client.stop();
  }
}

class TyraDapDescriptorFactory implements DebugAdapterDescriptorFactory {
  createDebugAdapterDescriptor(
    _session: DebugSession,
    _executable: DebugAdapterExecutable | undefined
  ): ProviderResult<DebugAdapterDescriptor> {
    const lldbDap = findLldbDap();
    if (lldbDap === undefined) {
      void window.showErrorMessage(
        "Tyra debugger: lldb-dap not found. " +
          "Install Xcode or LLVM, or set the LLDB_DAP_PATH environment variable."
      );
      return undefined;
    }
    return new DebugAdapterExecutable(lldbDap, []);
  }
}

function findLldbDap(): string | undefined {
  const candidates = [
    "/Applications/Xcode.app/Contents/Developer/usr/bin/lldb-dap",
    "/opt/homebrew/opt/llvm/bin/lldb-dap",
    "/opt/homebrew/opt/llvm@19/bin/lldb-dap",
    "/usr/local/opt/llvm/bin/lldb-dap",
    "/usr/bin/lldb-dap",
  ];
  const fromEnv = process.env["LLDB_DAP_PATH"];
  if (fromEnv !== undefined && fs.existsSync(fromEnv)) {
    return fromEnv;
  }
  return candidates.find((p) => fs.existsSync(p));
}
