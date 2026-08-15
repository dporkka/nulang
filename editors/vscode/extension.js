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
const { spawn } = require('child_process');

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

function runNulangCommand(args, cwd) {
    return new Promise((resolve, reject) => {
        const nulangPath = resolveNulangPath();
        const proc = spawn(nulangPath, args, { cwd, shell: true });
        
        let stdout = '';
        let stderr = '';
        
        proc.stdout.on('data', (data) => {
            stdout += data.toString();
        });
        
        proc.stderr.on('data', (data) => {
            stderr += data.toString();
        });
        
        proc.on('close', (code) => {
            if (code === 0) {
                resolve(stdout);
            } else {
                reject(new Error(stderr || `Process exited with code ${code}`));
            }
        });
        
        proc.on('error', (err) => {
            reject(err);
        });
    });
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

    // Register compile command
    context.subscriptions.push(
        vscode.commands.registerCommand('nulang.compile', async () => {
            const editor = vscode.window.activeTextEditor;
            if (!editor || editor.document.languageId !== 'nulang') {
                vscode.window.showErrorMessage('No Nulang file open');
                return;
            }
            
            const filePath = editor.document.uri.fsPath;
            const workspaceFolder = vscode.workspace.getWorkspaceFolder(editor.document.uri);
            const cwd = workspaceFolder ? workspaceFolder.uri.fsPath : path.dirname(filePath);
            
            try {
                await vscode.window.withProgress({
                    location: vscode.ProgressLocation.Notification,
                    title: "Compiling Nulang file...",
                    cancellable: false
                }, async () => {
                    const output = await runNulangCommand(['--backend', 'native', '--emit-nbc', filePath], cwd);
                    vscode.window.showInformationMessage(`Compiled: ${output.trim()}`);
                });
            } catch (err) {
                vscode.window.showErrorMessage(`Compile failed: ${err.message}`);
            }
        })
    );

    // Register run command
    context.subscriptions.push(
        vscode.commands.registerCommand('nulang.run', async () => {
            const editor = vscode.window.activeTextEditor;
            if (!editor || editor.document.languageId !== 'nulang') {
                vscode.window.showErrorMessage('No Nulang file open');
                return;
            }
            
            const filePath = editor.document.uri.fsPath;
            const workspaceFolder = vscode.workspace.getWorkspaceFolder(editor.document.uri);
            const cwd = workspaceFolder ? workspaceFolder.uri.fsPath : path.dirname(filePath);
            
            // Create output channel for run results
            const outputChannel = vscode.window.createOutputChannel('Nulang Run');
            outputChannel.show(true);
            outputChannel.appendLine(`Running ${filePath}...`);
            
            try {
                const output = await runNulangCommand(['--backend', 'native', filePath], cwd);
                outputChannel.appendLine(output);
                outputChannel.appendLine('\n--- Process completed successfully ---');
            } catch (err) {
                outputChannel.appendLine(`Error: ${err.message}`);
                outputChannel.appendLine('\n--- Process failed ---');
                vscode.window.showErrorMessage(`Run failed: ${err.message}`);
            }
        })
    );

    // Register check command
    context.subscriptions.push(
        vscode.commands.registerCommand('nulang.check', async () => {
            const editor = vscode.window.activeTextEditor;
            if (!editor || editor.document.languageId !== 'nulang') {
                vscode.window.showErrorMessage('No Nulang file open');
                return;
            }
            
            const filePath = editor.document.uri.fsPath;
            const workspaceFolder = vscode.workspace.getWorkspaceFolder(editor.document.uri);
            const cwd = workspaceFolder ? workspaceFolder.uri.fsPath : path.dirname(filePath);
            
            try {
                await vscode.window.withProgress({
                    location: vscode.ProgressLocation.Notification,
                    title: "Type checking Nulang file...",
                    cancellable: false
                }, async () => {
                    const output = await runNulangCommand(['--check', filePath], cwd);
                    vscode.window.showInformationMessage('Type check passed!');
                });
            } catch (err) {
                vscode.window.showErrorMessage(`Type check failed: ${err.message}`);
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
