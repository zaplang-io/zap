// Zap VS Code extension — a thin client for the `zap lsp` language server.
//
// All language intelligence (diagnostics, completion, hover, go-to-definition,
// document symbols) lives in the server, which this extension launches as a
// child process and talks to over stdio with the standard LSP client. The
// extension itself only resolves the server binary, starts the client, and
// exposes a "restart" command.

import * as path from "path";
import { existsSync } from "fs";
import {
  ExtensionContext,
  WorkspaceConfiguration,
  commands,
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

export async function activate(context: ExtensionContext): Promise<void> {
  context.subscriptions.push(
    commands.registerCommand("zap.restartServer", async () => {
      await restart(context);
    }),
  );

  await start(context);
}

export async function deactivate(): Promise<void> {
  if (client) {
    await client.stop();
    client = undefined;
  }
}

async function start(context: ExtensionContext): Promise<void> {
  const config = workspace.getConfiguration("zap");
  const command = resolveServerCommand(config);
  if (!command) {
    return;
  }

  // `zap lsp` speaks Content-Length-framed JSON-RPC over stdio.
  const serverOptions: ServerOptions = {
    run: { command, args: ["lsp"], transport: TransportKind.stdio },
    debug: { command, args: ["lsp"], transport: TransportKind.stdio },
  };

  const clientOptions: LanguageClientOptions = {
    // Match the `zap` language id contributed in package.json. `.zap` files on
    // disk and untitled buffers both activate the server.
    documentSelector: [
      { scheme: "file", language: "zap" },
      { scheme: "untitled", language: "zap" },
    ],
    synchronize: {
      fileEvents: workspace.createFileSystemWatcher("**/*.zap"),
    },
  };

  client = new LanguageClient(
    "zap",
    "Zap Language Server",
    serverOptions,
    clientOptions,
  );

  try {
    await client.start();
    context.subscriptions.push(client);
  } catch (err) {
    client = undefined;
    void window.showErrorMessage(
      `Zap: failed to start the language server (\`${command} lsp\`): ${String(
        err,
      )}`,
    );
  }
}

async function restart(context: ExtensionContext): Promise<void> {
  if (client) {
    await client.stop();
    client = undefined;
  }
  await start(context);
}

// Resolve the `zap` executable: an explicit `zap.lsp.serverPath` setting wins;
// otherwise fall back to `zap` on the user's PATH (resolved by the OS when the
// child process is spawned). If an explicit path is set but does not exist, we
// surface a clear, actionable error instead of letting the spawn fail silently.
function resolveServerCommand(
  config: WorkspaceConfiguration,
): string | undefined {
  const configured = config.get<string>("lsp.serverPath", "").trim();
  if (configured.length === 0) {
    return "zap";
  }

  const resolved = resolveConfiguredPath(configured);
  if (!existsSync(resolved)) {
    void window
      .showErrorMessage(
        `Zap: configured \`zap.lsp.serverPath\` does not exist: ${resolved}`,
        "Open Settings",
      )
      .then((choice) => {
        if (choice === "Open Settings") {
          void commands.executeCommand(
            "workbench.action.openSettings",
            "zap.lsp.serverPath",
          );
        }
      });
    return undefined;
  }
  return resolved;
}

// Expand a leading `~` and resolve a relative path against the first workspace
// folder, so `./build/zap` or `~/bin/zap` work as a user would expect.
function resolveConfiguredPath(configured: string): string {
  let p = configured;
  if (p === "~" || p.startsWith("~/")) {
    const home = process.env.HOME ?? process.env.USERPROFILE;
    if (home) {
      p = path.join(home, p.slice(1));
    }
  }
  if (!path.isAbsolute(p)) {
    const folder = workspace.workspaceFolders?.[0];
    if (folder && folder.uri.scheme === "file") {
      p = path.join(folder.uri.fsPath, p);
    }
  }
  return p;
}
