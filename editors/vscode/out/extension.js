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
exports.activate = activate;
exports.deactivate = deactivate;
const node_path_1 = __importDefault(require("node:path"));
const vscode = __importStar(require("vscode"));
const daemon_1 = require("./daemon");
const decorator_1 = require("./decorator");
let daemonClient;
let decorator;
let providerRegistration;
function activate(context) {
    const workspacePath = getWorkspacePath();
    if (!workspacePath) {
        return;
    }
    const config = vscode.workspace.getConfiguration("arcVcs");
    const arcCommand = config.get("arcCommand", "arc");
    daemonClient = new daemon_1.ArcDaemonClient(workspacePath, arcCommand);
    decorator = new decorator_1.ArcFileDecorator(daemonClient, workspacePath);
    providerRegistration = vscode.window.registerFileDecorationProvider(decorator);
    context.subscriptions.push(daemonClient, decorator, providerRegistration);
}
function deactivate() {
    providerRegistration?.dispose();
    providerRegistration = undefined;
    decorator?.dispose();
    decorator = undefined;
    daemonClient?.dispose();
    daemonClient = undefined;
}
function getWorkspacePath() {
    const folders = vscode.workspace.workspaceFolders;
    if (!folders || folders.length === 0) {
        void vscode.window.showWarningMessage("arc-vcs: open a workspace folder to enable decorations.");
        return undefined;
    }
    const firstFolder = folders[0];
    if (!firstFolder) {
        return undefined;
    }
    return node_path_1.default.normalize(firstFolder.uri.fsPath);
}
//# sourceMappingURL=extension.js.map