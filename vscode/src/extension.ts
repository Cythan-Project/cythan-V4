import * as fs from 'fs';
import * as path from 'path';
import * as vscode from 'vscode';
import {
  LanguageClient,
  LanguageClientOptions,
  ServerOptions,
  TransportKind,
} from 'vscode-languageclient/node';

let client: LanguageClient | undefined;

const DEFAULT_SERVER = 'cythan-lsp';

/** Resolve the server binary to launch.
 *
 *  Order of preference:
 *  1. User set `cythan.serverPath` to a non-default value: honor it.
 *     Absolute paths are taken as-is. Relative paths are resolved
 *     against the first workspace folder.
 *  2. Default value `cythan-lsp`: walk up from each open workspace
 *     folder looking for a Cargo workspace (a directory containing
 *     `Cargo.toml`); at each candidate try `target/release/cythan-lsp`
 *     then `target/debug/cythan-lsp`. This handles the common case
 *     where the user opens a sub-directory (e.g. `examples/new_syntax`)
 *     instead of the repo root.
 *  3. Fall back to `cythan-lsp` on PATH.
 */
function resolveServerPath(configured: string): string {
  const folders = vscode.workspace.workspaceFolders ?? [];
  const first = folders[0];

  if (configured && configured !== DEFAULT_SERVER) {
    if (path.isAbsolute(configured)) return configured;
    if (first) return path.resolve(first.uri.fsPath, configured);
    return configured;
  }

  const exe = process.platform === 'win32' ? '.exe' : '';
  for (const folder of folders) {
    const found = findInAncestors(folder.uri.fsPath, exe);
    if (found) return found;
  }
  return DEFAULT_SERVER;
}

/** Walk from `start` up to filesystem root looking for either
 *  `target/release/cythan-lsp` or `target/debug/cythan-lsp`. Prefer
 *  release. Returns the first hit, or null. */
function findInAncestors(start: string, exe: string): string | null {
  let dir = path.resolve(start);
  while (true) {
    for (const profile of ['release', 'debug']) {
      const candidate = path.join(dir, 'target', profile, `cythan-lsp${exe}`);
      try {
        if (fs.statSync(candidate).isFile()) return candidate;
      } catch { /* not here */ }
    }
    const parent = path.dirname(dir);
    if (parent === dir) return null;
    dir = parent;
  }
}

export async function activate(context: vscode.ExtensionContext): Promise<void> {
  const config = vscode.workspace.getConfiguration('cythan');
  const configured = config.get<string>('serverPath') || DEFAULT_SERVER;
  const serverPath = resolveServerPath(configured);

  const stdDir = config.get<string>('stdDir') || '';
  const initializationOptions: { stdDir?: string } = {};
  if (stdDir.length > 0) {
    initializationOptions.stdDir = stdDir;
  }

  const serverOptions: ServerOptions = {
    run: { command: serverPath, transport: TransportKind.stdio },
    debug: { command: serverPath, transport: TransportKind.stdio },
  };

  const clientOptions: LanguageClientOptions = {
    documentSelector: [{ scheme: 'file', language: 'cythan' }],
    initializationOptions,
    synchronize: {
      configurationSection: 'cythan',
      fileEvents: vscode.workspace.createFileSystemWatcher('**/*.ct'),
    },
  };

  client = new LanguageClient(
    'cythan',
    'Cythan Language Server',
    serverOptions,
    clientOptions
  );

  context.subscriptions.push({
    dispose: () => {
      client?.stop();
    },
  });

  try {
    await client.start();
  } catch (err) {
    const hint =
      `Build it with \`cargo build --release -p cythan-lsp\` from the ` +
      `cythan-V4 repo root, then reload the window. The extension searches ` +
      `\`target/{release,debug}/cythan-lsp\` from each workspace folder up ` +
      `to the filesystem root. If your binary lives elsewhere, set ` +
      `"cythan.serverPath" in settings to its absolute path.`;
    void vscode.window.showErrorMessage(
      `Failed to start Cythan language server: tried \`${serverPath}\`. ${hint} (${err})`
    );
  }
}

export function deactivate(): Thenable<void> | undefined {
  return client?.stop();
}
