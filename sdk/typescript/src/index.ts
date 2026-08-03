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

export { Sandbox } from "./sandbox.ts";
export type { ExecOptions, LogsOptions } from "./sandbox.ts";

export { createSh, raw, CommandResult, ShellPromise } from "./shell.ts";
export type { Sh, ShellRunOptions, ShellRunner } from "./shell.ts";

export { images, listImages } from "./images.ts";
export { volumes, listVolumes, removeVolume } from "./volumes.ts";
export { probe, fetchImage, versions, growDisk } from "./system.ts";

export { setBinaryPath, resolveBinary, resetBinaryCache } from "./binary.ts";
export { ensurePreflight, resetPreflight } from "./preflight.ts";

export {
  BsdkrunError,
  BinaryNotFoundError,
  CommandFailedError,
  SandboxNotFoundError,
} from "./errors.ts";

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
} from "./types.ts";
