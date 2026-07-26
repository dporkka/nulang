// Nulang VS Code Extension — LSP Client
//
// Launches `nulang --lsp` as the language server and wires it to the
// VS Code Language Client API for diagnostics, goto-definition,
// references, hover, completion, and more.
//
// Install: copy this directory to ~/.vscode/extensions/nulang/
//   or run:  npx @vscode/vsce package && code --install-extension nulang-0.1.0.vsix

const vscode = require('vscode');
const path = require('path');
const { LanguageClient, TransportKind } = require('vscode-languageclient/node');

/** @type {LanguageClient} */
let client;

/**
 * Resolve the `nulang` binary path.
 *
 * Checks NULANG_PATH env var first, then PATH, then falls back to
 * `nulang` (hoping it's on the user's PATH).
 */
function resolveNulangPath() {
    const envPath = process.env.NULANG_PATH;
    if (envPath) return envPath;
    return 'nulang';
}

async function activate(context) {
    const serverPath = resolveNulangPath();

    const serverOptions = {
        command: serverPath,
        args: ['--lsp'],
        transport: TransportKind.stdio
    };

    const clientOptions = {
        documentSelector: [{ scheme: 'file', language: 'nulang' }],
        synchronize: {
            fileEvents: vscode.workspace.createFileSystemWatcher('**/*.nula')
        }
    };

    client = new LanguageClient(
        'nulang-lsp',
        'Nulang Language Server',
        serverOptions,
        clientOptions
    );

    await client.start();

    // Register restart command
    context.subscriptions.push(
        vscode.commands.registerCommand('nulang.restartServer', async () => {
            if (client) {
                await client.stop();
                await client.start();
                vscode.window.showInformationMessage('Nulang language server restarted');
            }
        })
    );
}

async function deactivate() {
    if (client) {
        await client.stop();
    }
}

module.exports = { activate, deactivate };
