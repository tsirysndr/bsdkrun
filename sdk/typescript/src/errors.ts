import type { CommandResult } from "./shell.js";

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

// ---- remote Client (GraphQL) errors -----------------------------------------
//
// Thrown by `Client` (client.ts) instead of the CommandFailedError family
// above, which is specific to shelling out to the local `bsdkrun` binary.
// Mirrors web/src/lib/graphql.ts's `GraphQLError`/`AuthError` — same names,
// same meaning, so error handling reads the same whether the daemon is local
// or remote.

/** A GraphQL request failed: a transport error, or a non-auth `errors[]` entry. */
export class GraphQLError extends BsdkrunError {
  /** The error's `extensions.code`, when the daemon set one. */
  readonly code?: string;

  constructor(message: string, code?: string) {
    super(message);
    this.name = "GraphQLError";
    this.code = code;
  }
}

/** The daemon rejected the bearer token — over HTTP (401) or the WS handshake. */
export class AuthError extends GraphQLError {
  constructor(message = "the daemon rejected this token") {
    super(message, "UNAUTHENTICATED");
    this.name = "AuthError";
  }
}
