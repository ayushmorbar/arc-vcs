import path from "node:path";
import * as vscode from "vscode";

import { ArcDaemonClient, FileState } from "./daemon";

const STATUS_COLORS: Record<FileState["status"], vscode.ThemeColor> = {
  conflict: new vscode.ThemeColor("arc.decorations.conflictForeground"),
  ai_generated: new vscode.ThemeColor("arc.decorations.aiForeground"),
  modified: new vscode.ThemeColor("arc.decorations.modifiedForeground"),
  untracked: new vscode.ThemeColor("arc.decorations.untrackedForeground"),
};

export class ArcFileDecorator implements vscode.FileDecorationProvider, vscode.Disposable {
  private readonly changedEmitter = new vscode.EventEmitter<vscode.Uri | vscode.Uri[] | undefined>();
  public readonly onDidChangeFileDecorations = this.changedEmitter.event;

  private readonly fileStates = new Map<string, FileState["status"]>();
  private refreshInFlight: Promise<void> | undefined;

  private readonly notificationSubscription: vscode.Disposable;

  public constructor(
    private readonly daemonClient: ArcDaemonClient,
    private readonly workspacePath: string
  ) {
    this.notificationSubscription = this.daemonClient.onDidFileDecorationsChange(() => {
      void this.refreshStates()
        .then(() => {
          this.changedEmitter.fire(undefined);
        })
        .catch((error: unknown) => {
          console.error("[arc-vcs] failed to refresh file decorations", error);
        });
    });
  }

  public async provideFileDecoration(uri: vscode.Uri): Promise<vscode.FileDecoration | undefined> {
    try {
      await this.refreshStates();
    } catch (error) {
      console.error("[arc-vcs] decoration refresh failed", error);
      return undefined;
    }

    const key = this.toWorkspaceRelativePath(uri);
    if (!key) {
      return undefined;
    }

    const status = this.fileStates.get(key);
    if (!status) {
      return undefined;
    }

    switch (status) {
      case "conflict":
        return new vscode.FileDecoration("C", "Mathematical Conflict", STATUS_COLORS.conflict);
      case "ai_generated":
        return new vscode.FileDecoration("AI", "Signed by Author::AI", STATUS_COLORS.ai_generated);
      case "modified":
        return new vscode.FileDecoration("M", "Modified", STATUS_COLORS.modified);
      case "untracked":
        return new vscode.FileDecoration("U", "Untracked", STATUS_COLORS.untracked);
      default:
        return undefined;
    }
  }

  public dispose(): void {
    this.notificationSubscription.dispose();
    this.changedEmitter.dispose();
  }

  private async refreshStates(): Promise<void> {
    if (!this.refreshInFlight) {
      this.refreshInFlight = this.doRefresh().finally(() => {
        this.refreshInFlight = undefined;
      });
    }
    return this.refreshInFlight;
  }

  private async doRefresh(): Promise<void> {
    const states = await this.daemonClient.getFileStates();
    this.fileStates.clear();

    for (const state of states) {
      this.fileStates.set(path.normalize(state.file_path), state.status);
    }
  }

  private toWorkspaceRelativePath(uri: vscode.Uri): string | undefined {
    if (uri.scheme !== "file") {
      return undefined;
    }

    const relative = path.relative(this.workspacePath, uri.fsPath);
    if (relative.startsWith("..") || path.isAbsolute(relative)) {
      return undefined;
    }

    return path.normalize(relative);
  }
}
