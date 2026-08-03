import { CommandFailedError } from "./errors.ts";
import { runCli } from "./process.ts";
import { CommandResult } from "./shell.ts";

function check(
  res: { stdout: string; stderr: string; exitCode: number },
  label: string,
): string {
  if (res.exitCode !== 0) {
    throw new CommandFailedError(
      new CommandResult(res.stdout, res.stderr, res.exitCode, label),
    );
  }
  return res.stdout;
}

/**
 * Sanity-check the toolchain: verify libkrun links and a context can be
 * created/configured. Does not boot. Returns true on success.
 */
export async function probe(): Promise<boolean> {
  const res = await runCli(["probe"]);
  return res.exitCode === 0;
}

/** Download + prepare a BSD image ahead of time. Returns the command output. */
export async function fetchImage(
  os: "freebsd" | "netbsd",
  opts: { version?: string; dir?: string; force?: boolean } = {},
): Promise<string> {
  const args = ["fetch", "--os", os];
  if (opts.version) args.push("--version", opts.version);
  if (opts.dir) args.push("--dir", opts.dir);
  if (opts.force) args.push("--force");
  return check(await runCli(args), "bsdkrun fetch");
}

/** List the arm64 builds available to fetch for a BSD. */
export async function versions(
  os: "freebsd" | "netbsd",
): Promise<string> {
  return check(await runCli(["versions", "--os", os]), "bsdkrun versions");
}

/** Grow a raw disk image (the guest expands its root FS on next boot). */
export async function growDisk(disk: string, size: string): Promise<void> {
  check(
    await runCli(["grow", "--disk", disk, "--size", size]),
    "bsdkrun grow",
  );
}
