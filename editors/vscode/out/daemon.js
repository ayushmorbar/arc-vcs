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
Object.defineProperty(exports, "__esModule", { value: true });
exports.ArcDaemonClient = void 0;
const node_child_process_1 = require("node:child_process");
const vscode = __importStar(require("vscode"));
const REQUEST_TIMEOUT_MS = 5000;
class ArcDaemonClient {
    workspacePath;
    daemonProcess;
    pending = new Map();
    nextId = 1;
    stdoutBuffer = "";
    decorationsChangedEmitter = new vscode.EventEmitter();
    onDidFileDecorationsChange = this.decorationsChangedEmitter.event;
    constructor(workspacePath, arcCommand) {
        this.workspacePath = workspacePath;
        this.daemonProcess = (0, node_child_process_1.spawn)(arcCommand, ["daemon"], {
            cwd: workspacePath,
            stdio: "pipe",
            windowsHide: true,
        });
        this.daemonProcess.stdout.setEncoding("utf8");
        this.daemonProcess.stdout.on("data", (chunk) => {
            this.handleStdoutChunk(chunk.toString());
        });
        this.daemonProcess.stderr.setEncoding("utf8");
        this.daemonProcess.stderr.on("data", (chunk) => {
            console.error("[arc-vcs] daemon stderr:", chunk.toString().trim());
        });
        this.daemonProcess.on("error", (error) => {
            this.rejectAllPending(error);
        });
        this.daemonProcess.on("exit", (code, signal) => {
            const reason = new Error(`arc daemon exited (code=${code ?? "null"}, signal=${signal ?? "null"})`);
            this.rejectAllPending(reason);
        });
    }
    async getFileStates(path) {
        const raw = await this.sendRequest("get_file_states", { path });
        if (!isFileStateArray(raw)) {
            throw new Error("arc daemon returned invalid get_file_states payload");
        }
        return raw;
    }
    dispose() {
        this.decorationsChangedEmitter.dispose();
        this.rejectAllPending(new Error("arc daemon client disposed"));
        if (!this.daemonProcess.killed) {
            this.daemonProcess.kill();
        }
    }
    async sendRequest(method, params) {
        const id = this.nextId++;
        const payload = {
            jsonrpc: "2.0",
            id,
            method,
            params,
        };
        return new Promise((resolve, reject) => {
            const timer = setTimeout(() => {
                this.pending.delete(id);
                reject(new Error(`arc daemon request timed out: ${method}`));
            }, REQUEST_TIMEOUT_MS);
            this.pending.set(id, {
                resolve: (value) => resolve(value),
                reject,
                timer,
            });
            const line = `${JSON.stringify(payload)}\n`;
            this.daemonProcess.stdin.write(line, "utf8", (error) => {
                if (error) {
                    clearTimeout(timer);
                    this.pending.delete(id);
                    reject(error);
                }
            });
        });
    }
    handleStdoutChunk(chunk) {
        this.stdoutBuffer += chunk;
        let newlineIndex = this.stdoutBuffer.indexOf("\n");
        while (newlineIndex >= 0) {
            const rawLine = this.stdoutBuffer.slice(0, newlineIndex).trim();
            this.stdoutBuffer = this.stdoutBuffer.slice(newlineIndex + 1);
            if (rawLine.length > 0) {
                this.handleJsonLine(rawLine);
            }
            newlineIndex = this.stdoutBuffer.indexOf("\n");
        }
    }
    handleJsonLine(rawLine) {
        let parsed;
        try {
            parsed = JSON.parse(rawLine);
        }
        catch (error) {
            console.error("[arc-vcs] failed to parse daemon output", error);
            return;
        }
        if (this.isResponse(parsed)) {
            const pending = this.pending.get(parsed.id);
            if (!pending) {
                return;
            }
            this.pending.delete(parsed.id);
            clearTimeout(pending.timer);
            if (parsed.error) {
                pending.reject(new Error(parsed.error.message));
                return;
            }
            pending.resolve(parsed.result);
            return;
        }
        if (this.isNotification(parsed) && parsed.method === "arc/fileDecorationsChanged") {
            const notification = parsed;
            const changedPath = notification.params && typeof notification.params.path === "string"
                ? notification.params.path
                : undefined;
            this.decorationsChangedEmitter.fire(changedPath);
        }
    }
    isResponse(value) {
        if (typeof value !== "object" || value === null) {
            return false;
        }
        const candidate = value;
        return candidate.jsonrpc === "2.0" && typeof candidate.id === "number";
    }
    isNotification(value) {
        if (typeof value !== "object" || value === null) {
            return false;
        }
        const candidate = value;
        return candidate.jsonrpc === "2.0" && typeof candidate.method === "string";
    }
    rejectAllPending(reason) {
        for (const request of this.pending.values()) {
            clearTimeout(request.timer);
            request.reject(reason);
        }
        this.pending.clear();
    }
}
exports.ArcDaemonClient = ArcDaemonClient;
function isFileStateArray(value) {
    if (!Array.isArray(value)) {
        return false;
    }
    return value.every((entry) => {
        if (typeof entry !== "object" || entry === null) {
            return false;
        }
        const candidate = entry;
        const status = candidate.status;
        return (typeof candidate.file_path === "string" &&
            (status === "conflict" ||
                status === "ai_generated" ||
                status === "modified" ||
                status === "untracked"));
    });
}
//# sourceMappingURL=daemon.js.map