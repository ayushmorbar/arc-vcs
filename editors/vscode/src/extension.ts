import path from "node:path";
import * as vscode from "vscode";

import { ArcDaemonClient } from "./daemon";
import { ArcFileDecorator } from "./decorator";

let daemonClient: ArcDaemonClient | undefined;
let decorator: ArcFileDecorator | undefined;
let providerRegistration: vscode.Disposable | undefined;

export function activate(context: vscode.ExtensionContext): void {
  const workspacePath = getWorkspacePath();
  if (!workspacePath) {
    return;
  }

  const config = vscode.workspace.getConfiguration("arcVcs");
  const arcCommand = config.get<string>("arcCommand", "arc");

  daemonClient = new ArcDaemonClient(workspacePath, arcCommand);
  decorator = new ArcFileDecorator(daemonClient, workspacePath);
  providerRegistration = vscode.window.registerFileDecorationProvider(decorator);

  context.subscriptions.push(daemonClient, decorator, providerRegistration);
}

export function deactivate(): void {
  providerRegistration?.dispose();
  providerRegistration = undefined;

  decorator?.dispose();
  decorator = undefined;

  daemonClient?.dispose();
  daemonClient = undefined;
}

function getWorkspacePath(): string | undefined {
  const folders = vscode.workspace.workspaceFolders;
  if (!folders || folders.length === 0) {
    void vscode.window.showWarningMessage("arc-vcs: open a workspace folder to enable decorations.");
    return undefined;
  }

  const firstFolder = folders[0];
  if (!firstFolder) {
    return undefined;
  }

  return path.normalize(firstFolder.uri.fsPath);
}
