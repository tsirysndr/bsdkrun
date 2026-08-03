import type { CommandResult } from "./shell.ts";

/** Base class for every error the SDK throws. */
export class BsdkrunError extends Error {
  constructor(message: string) {
    super(message);
    this.name = "BsdkrunError";
  }
}

/** The `bsdkrun` binary could not be located on the host. */
export class BinaryNotFoundError extends BsdkrunError {
  constructor(searched: string[]) {
    super(
      `could not find the "bsdkrun" binary. Set BSDKRUN_BIN, add it to PATH, ` +
        `or call setBinaryPath(). Looked in: ${searched.join(", ")}`,
    );
    this.name = "BinaryNotFoundError";
  }
}

/** A `bsdkrun` invocation exited non-zero. */
export class CommandFailedError extends BsdkrunError {
  readonly exitCode: number;
  readonly stdout: string;
  readonly stderr: string;
  readonly command: string;

  constructor(result: CommandResult) {
    super(
      `command failed (exit ${result.exitCode}): ${result.command}` +
        (result.stderr.trim() ? `\n${result.stderr.trim()}` : ""),
    );
    this.name = "CommandFailedError";
    this.exitCode = result.exitCode;
    this.stdout = result.stdout;
    this.stderr = result.stderr;
    this.command = result.command;
  }
}

/** No machine matched the given id / prefix. */
export class SandboxNotFoundError extends BsdkrunError {
  constructor(id: string) {
    super(`no sandbox found matching id ${JSON.stringify(id)}`);
    this.name = "SandboxNotFoundError";
  }
}
