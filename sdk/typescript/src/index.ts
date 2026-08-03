/**
 * @bsdkrun/sdk — a TypeScript SDK for {@link https://github.com/tsirysndr/bsdkrun | bsdkrun},
 * a Firecracker-style microVM launcher for BSD and Linux guests. Inspired by the
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

export { runCli, spawnCli } from "./process.js";
export type { RunOptions, RawResult, SpawnOptions } from "./process.js";

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
  BaseCreateOptions,
  DiskPersistenceOptions,
  NetworkOptions,
  ResourceOptions,
  PortForward,
  GuestKind,
  SandboxInfo,
  ImageInfo,
  VolumeInfo,
} from "./types.js";
