import { ChildProcessWithoutNullStreams, spawn } from "node:child_process";
import * as vscode from "vscode";

type JsonRpcId = number;

interface JsonRpcRequest<TParams> {
  readonly jsonrpc: "2.0";
  readonly id: JsonRpcId;
  readonly method: string;
  readonly params: TParams;
}

interface JsonRpcError {
  readonly code: number;
  readonly message: string;
}

interface JsonRpcResponse<TResult> {
  readonly jsonrpc: "2.0";
  readonly id: JsonRpcId;
  readonly result?: TResult;
  readonly error?: JsonRpcError;
}

interface JsonRpcNotification<TParams> {
  readonly jsonrpc: "2.0";
  readonly method: string;
  readonly params?: TParams;
}

interface FileDecorationsChangedParams {
  readonly path?: string;
}

export interface FileState {
  readonly file_path: string;
  readonly status: "conflict" | "ai_generated" | "modified" | "untracked";
}

interface GetFileStatesParams {
  readonly path: string;
}

interface PendingRequest {
  readonly resolve: (value: unknown) => void;
  readonly reject: (error: Error) => void;
  readonly timer: NodeJS.Timeout;
}

const REQUEST_TIMEOUT_MS = 5000;

export class ArcDaemonClient implements vscode.Disposable {
  private readonly daemonProcess: ChildProcessWithoutNullStreams;
  private readonly pending = new Map<JsonRpcId, PendingRequest>();
  private nextId = 1;
  private stdoutBuffer = "";
  private readonly decorationsChangedEmitter =
    new vscode.EventEmitter<string | undefined>();

  public readonly onDidFileDecorationsChange = this.decorationsChangedEmitter.event;

  public constructor(private readonly workspacePath: string, arcCommand: string) {
    this.daemonProcess = spawn(arcCommand, ["daemon"], {
      cwd: workspacePath,
      stdio: "pipe",
      windowsHide: true,
    });

    this.daemonProcess.stdout.setEncoding("utf8");
    this.daemonProcess.stdout.on("data", (chunk: string | Buffer) => {
      this.handleStdoutChunk(chunk.toString());
    });

    this.daemonProcess.stderr.setEncoding("utf8");
    this.daemonProcess.stderr.on("data", (chunk: string | Buffer) => {
      console.error("[arc-vcs] daemon stderr:", chunk.toString().trim());
    });

    this.daemonProcess.on("error", (error: Error) => {
      this.rejectAllPending(error);
    });

    this.daemonProcess.on("exit", (code: number | null, signal: NodeJS.Signals | null) => {
      const reason = new Error(
        `arc daemon exited (code=${code ?? "null"}, signal=${signal ?? "null"})`
      );
      this.rejectAllPending(reason);
    });
  }

  public async getFileStates(path: string): Promise<readonly FileState[]> {
    const raw = await this.sendRequest<GetFileStatesParams, unknown>(
      "get_file_states",
      { path }
    );
    if (!isFileStateArray(raw)) {
      throw new Error("arc daemon returned invalid get_file_states payload");
    }
    return raw;
  }

  public dispose(): void {
    this.decorationsChangedEmitter.dispose();
    this.rejectAllPending(new Error("arc daemon client disposed"));
    if (!this.daemonProcess.killed) {
      this.daemonProcess.kill();
    }
  }

  private async sendRequest<TParams, TResult>(
    method: string,
    params: TParams
  ): Promise<TResult> {
    const id = this.nextId++;
    const payload: JsonRpcRequest<TParams> = {
      jsonrpc: "2.0",
      id,
      method,
      params,
    };

    return new Promise<TResult>((resolve, reject) => {
      const timer = setTimeout(() => {
        this.pending.delete(id);
        reject(new Error(`arc daemon request timed out: ${method}`));
      }, REQUEST_TIMEOUT_MS);

      this.pending.set(id, {
        resolve: (value: unknown) => resolve(value as TResult),
        reject,
        timer,
      });
      const line = `${JSON.stringify(payload)}\n`;
      this.daemonProcess.stdin.write(line, "utf8", (error?: Error | null) => {
        if (error) {
          clearTimeout(timer);
          this.pending.delete(id);
          reject(error);
        }
      });
    });
  }

  private handleStdoutChunk(chunk: string): void {
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

  private handleJsonLine(rawLine: string): void {
    let parsed: unknown;
    try {
      parsed = JSON.parse(rawLine);
    } catch (error) {
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
      const notification = parsed as JsonRpcNotification<FileDecorationsChangedParams>;
      const changedPath =
        notification.params && typeof notification.params.path === "string"
          ? notification.params.path
          : undefined;
      this.decorationsChangedEmitter.fire(changedPath);
    }
  }

  private isResponse(value: unknown): value is JsonRpcResponse<unknown> {
    if (typeof value !== "object" || value === null) {
      return false;
    }
    const candidate = value as Record<string, unknown>;
    return candidate.jsonrpc === "2.0" && typeof candidate.id === "number";
  }

  private isNotification(value: unknown): value is JsonRpcNotification<unknown> {
    if (typeof value !== "object" || value === null) {
      return false;
    }
    const candidate = value as Record<string, unknown>;
    return candidate.jsonrpc === "2.0" && typeof candidate.method === "string";
  }

  private rejectAllPending(reason: Error): void {
    for (const request of this.pending.values()) {
      clearTimeout(request.timer);
      request.reject(reason);
    }
    this.pending.clear();
  }
}

function isFileStateArray(value: unknown): value is readonly FileState[] {
  if (!Array.isArray(value)) {
    return false;
  }

  return value.every((entry) => {
    if (typeof entry !== "object" || entry === null) {
      return false;
    }
    const candidate = entry as Record<string, unknown>;
    const status = candidate.status;
    return (
      typeof candidate.file_path === "string" &&
      (status === "conflict" ||
        status === "ai_generated" ||
        status === "modified" ||
        status === "untracked")
    );
  });
}
