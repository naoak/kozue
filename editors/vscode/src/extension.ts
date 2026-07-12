import * as path from "path";
import * as vscode from "vscode";
import {
  LanguageClient,
  LanguageClientOptions,
  ServerOptions,
  TransportKind,
} from "vscode-languageclient/node";

let client: LanguageClient | undefined;

export function activate(context: vscode.ExtensionContext): void {
  const config = vscode.workspace.getConfiguration("kozue");
  const serverPath: string = config.get("serverPath") ?? "kozue-lsp";

  const serverOptions: ServerOptions = {
    command: serverPath,
    transport: TransportKind.stdio,
  };

  const clientOptions: LanguageClientOptions = {
    documentSelector: [
      { scheme: "file", language: "kozue" },
      { scheme: "file", language: "mermaid" },
      { scheme: "file", language: "plantuml" },
    ],
    synchronize: {
      fileEvents: vscode.workspace.createFileSystemWatcher(
        "**/*.{kozue,kzd,mmd,mermaid,puml,plantuml,pu,iuml}"
      ),
    },
  };

  client = new LanguageClient(
    "kozue-lsp",
    "Kozue Language Server",
    serverOptions,
    clientOptions
  );

  client.start();
  context.subscriptions.push({
    dispose: () => client?.stop(),
  });
}

export function deactivate(): Thenable<void> | undefined {
  return client?.stop();
}
