"use strict";
var __createBinding = (this && this.__createBinding) || (Object.create ? (function(o, m, k, k2) {
    if (k2 === undefined) k2 = k;
    var desc = Object.getOwnPropertyDescriptor(m, k);
    if (!desc || ("get" in desc ? !m.__esModule : desc.writable || desc.configurable)) {
      desc = { enumerable: true, get: function() { return m[k]; } };
    }
    Object.defineProperty(o, k2, desc);
}) : (function(o, m, k, k2) {
    if (k2 === undefined) k2 = k;
    o[k2] = m[k];
}));
var __setModuleDefault = (this && this.__setModuleDefault) || (Object.create ? (function(o, v) {
    Object.defineProperty(o, "default", { enumerable: true, value: v });
}) : function(o, v) {
    o["default"] = v;
});
var __importStar = (this && this.__importStar) || (function () {
    var ownKeys = function(o) {
        ownKeys = Object.getOwnPropertyNames || function (o) {
            var ar = [];
            for (var k in o) if (Object.prototype.hasOwnProperty.call(o, k)) ar[ar.length] = k;
            return ar;
        };
        return ownKeys(o);
    };
    return function (mod) {
        if (mod && mod.__esModule) return mod;
        var result = {};
        if (mod != null) for (var k = ownKeys(mod), i = 0; i < k.length; i++) if (k[i] !== "default") __createBinding(result, mod, k[i]);
        __setModuleDefault(result, mod);
        return result;
    };
})();
var __importDefault = (this && this.__importDefault) || function (mod) {
    return (mod && mod.__esModule) ? mod : { "default": mod };
};
Object.defineProperty(exports, "__esModule", { value: true });
exports.ArcFileDecorator = void 0;
const node_path_1 = __importDefault(require("node:path"));
const vscode = __importStar(require("vscode"));
const STATUS_COLORS = {
    conflict: new vscode.ThemeColor("arc.decorations.conflictForeground"),
    ai_generated: new vscode.ThemeColor("arc.decorations.aiForeground"),
    modified: new vscode.ThemeColor("arc.decorations.modifiedForeground"),
    untracked: new vscode.ThemeColor("arc.decorations.untrackedForeground"),
};
class ArcFileDecorator {
    daemonClient;
    workspacePath;
    changedEmitter = new vscode.EventEmitter();
    onDidChangeFileDecorations = this.changedEmitter.event;
    fileStates = new Map();
    refreshInFlight;
    notificationSubscription;
    constructor(daemonClient, workspacePath) {
        this.daemonClient = daemonClient;
        this.workspacePath = workspacePath;
        this.notificationSubscription = this.daemonClient.onDidFileDecorationsChange(() => {
            void this.refreshStates()
                .then(() => {
                this.changedEmitter.fire(undefined);
            })
                .catch((error) => {
                console.error("[arc-vcs] failed to refresh file decorations", error);
            });
        });
    }
    async provideFileDecoration(uri) {
        try {
            await this.refreshStates();
        }
        catch (error) {
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
    dispose() {
        this.notificationSubscription.dispose();
        this.changedEmitter.dispose();
    }
    async refreshStates() {
        if (!this.refreshInFlight) {
            this.refreshInFlight = this.doRefresh().finally(() => {
                this.refreshInFlight = undefined;
            });
        }
        return this.refreshInFlight;
    }
    async doRefresh() {
        const states = await this.daemonClient.getFileStates();
        this.fileStates.clear();
        for (const state of states) {
            this.fileStates.set(node_path_1.default.normalize(state.file_path), state.status);
        }
    }
    toWorkspaceRelativePath(uri) {
        if (uri.scheme !== "file") {
            return undefined;
        }
        const relative = node_path_1.default.relative(this.workspacePath, uri.fsPath);
        if (relative.startsWith("..") || node_path_1.default.isAbsolute(relative)) {
            return undefined;
        }
        return node_path_1.default.normalize(relative);
    }
}
exports.ArcFileDecorator = ArcFileDecorator;
//# sourceMappingURL=decorator.js.map