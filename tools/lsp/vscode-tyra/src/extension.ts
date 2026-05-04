import { ExtensionContext, workspace } from "vscode";
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
      fileEvents: workspace.createFileSystemWatcher("**/*.tyra"),
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
}

export async function deactivate(): Promise<void> {
  if (client) {
    await client.stop();
  }
}
