/**
 * @bsdkrun/sdk — a TypeScript SDK for {@link https://github.com/tsirysndr/bsdkrun | bsdkrun},
 * a Firecracker-style microVM launcher for BSD, Linux, and unikernel guests. Inspired by the
 * Vercel and Deno Sandbox SDKs. Runs on Node.js, Deno and Bun.
 *
 * ```ts
 * import { Sandbox } from "@bsdkrun/sdk";
 *
 * const box = await Sandbox.create({ os: "linux", image: "alpine" });
 * const out = await box.sh`uname -a`.text();
 * await box.exec(["apk", "add", "curl"]);
 * await box.stop();
 * ```
 */

export { Sandbox } from "./sandbox.js";
export type {
  ExecOptions,
  LogsOptions,
  SshSetupOptions,
  TailscaleUpOptions,
} from "./sandbox.js";

export { Terminal } from "./terminal.js";
export type {
  TerminalOptions,
  DataSink,
  WebSocketLike,
} from "./terminal.js";

export {
  AgentConnection,
  AgentUnavailableError,
  readAgentPort,
} from "./agent-protocol.js";
export type { AgentConnectOptions, AgentHandlers } from "./agent-protocol.js";

export { createSh, raw, CommandResult, ShellPromise } from "./shell.js";
export type { Sh, ShellRunOptions, ShellRunner } from "./shell.js";

export { images, listImages } from "./images.js";
export { volumes, listVolumes, removeVolume } from "./volumes.js";
export {
  networks,
  listNetworks,
  createNetwork,
  removeNetwork,
  connectNetwork,
  disconnectNetwork,
  syncNetwork,
  networkMembers,
} from "./networks.js";
export { probe, fetchImage, versions, growDisk } from "./system.js";

export { setBinaryPath, resolveBinary, resetBinaryCache } from "./binary.js";
export {
  ensurePreflight,
  resetPreflight,
  cachedPreflightEnv,
  libkrunTarballUrl,
  linuxArchSlug,
} from "./preflight.js";
export type { PreflightEnv } from "./preflight.js";

export { runCli, runCliBinary, spawnCli } from "./process.js";
export type {
  RunOptions,
  RawResult,
  BinaryResult,
  SpawnOptions,
} from "./process.js";

export { FileSystem, FileTransferError } from "./filesystem.js";
export type { FsOptions, DownloadOptions } from "./filesystem.js";

export { buildCreateArgs } from "./args.js";

export {
  BsdkrunError,
  BinaryNotFoundError,
  CommandFailedError,
  SandboxNotFoundError,
} from "./errors.js";

export type {
  CreateOptions,
  LinuxCreateOptions,
  FreebsdCreateOptions,
  NetbsdCreateOptions,
  FirmwareCreateOptions,
  KernelCreateOptions,
  UnikraftCreateOptions,
  Solo5CreateOptions,
  NanosCreateOptions,
  OsvCreateOptions,
  BaseCreateOptions,
  DiskPersistenceOptions,
  NetworkOptions,
  ResourceOptions,
  PortForward,
  GuestKind,
  SandboxInfo,
  ImageInfo,
  VolumeInfo,
  NetworkInfo,
} from "./types.js";

// ---- remote Client (GraphQL) ------------------------------------------------
//
// Talks to a remote `bsdkrund` daemon's GraphQL API instead of shelling out to
// a local `bsdkrun` binary — see client.ts. `types.ts`'s `CommandResult` (a
// plain `{exitCode, stdout, stderr}` record, the GraphQL mutation result) is
// re-exported here as `RemoteCommandResult`: it shares a name with — but is
// not the same type as — the CLI-flavored `CommandResult` *class* already
// exported below from shell.ts, and a module can't export two members under
// one identifier. Within client.ts itself the unaliased name is used, since
// that file never imports shell.ts's `CommandResult`.

export { Client, normalizeUrl, URL_ENV, TOKEN_ENV } from "./client.js";
export type {
  ShellSession,
  NetOptions,
  BsdOsInput,
  RunLinuxOptions,
  RunBsdOptions,
  RunNanosOptions,
  RunUnikraftOptions,
  RunSolo5Options,
  RunOsvOptions,
  RunFlavorOptions,
} from "./client.js";

export { fromGraphQLMachine } from "./sandbox.js";

export { GraphQLError, AuthError } from "./errors.js";

export type {
  CommandResult as RemoteCommandResult,
  ShellSessionInfo,
  ShellOutput,
} from "./types.js";
