import type {
  CreateOptions,
  DiskPersistenceOptions,
  NetworkOptions,
  ResourceOptions,
} from "./types.js";

function netArgs(net: NetworkOptions | undefined): string[] {
  const a: string[] = [];
  if (!net) return a;
  if (net.disabled) a.push("--no-net");
  for (const p of net.ports ?? []) {
    a.push("--port", typeof p === "string" ? p : `${p.host}:${p.guest}`);
  }
  if (net.mac) a.push("--mac", net.mac);
  if (net.network) a.push("--network", net.network);
  return a;
}

function nameArgs(o: { name?: string }): string[] {
  return o.name ? ["--name", o.name] : [];
}

function vmArgs(o: ResourceOptions): string[] {
  const a: string[] = [];
  if (o.cpus != null) a.push("--cpus", String(o.cpus));
  if (o.mem != null) a.push("--mem", String(o.mem));
  return a;
}

function diskArgs(o: DiskPersistenceOptions): string[] {
  const a: string[] = [];
  if (o.persist) a.push("--persist");
  if (o.volume) a.push("-v", o.volume);
  for (const d of o.attachDisk ?? []) a.push("--attach-disk", d);
  return a;
}

/**
 * Build the full `bsdkrun` argv (minus the binary and global flags) for a
 * detached `create`. Every path runs with `-d` so `create` yields a handle.
 */
export function buildCreateArgs(opts: CreateOptions): string[] {
  switch (opts.os) {
    case "linux": {
      const a = ["linux", opts.image, "-d"];
      if (opts.kernel) a.push("--kernel", opts.kernel);
      if (opts.kernelVersion) a.push("--kernel-version", opts.kernelVersion);
      if (opts.initramfs) a.push("--initramfs");
      if (opts.volume) a.push("-v", opts.volume);
      for (const m of opts.mounts ?? []) a.push("--mount", m);
      if (opts.entrypoint) a.push("--entrypoint", opts.entrypoint);
      if (opts.console) a.push("--console", opts.console);
      a.push(...netArgs(opts.net), ...nameArgs(opts), ...vmArgs(opts));
      if (opts.command?.length) a.push("--", ...opts.command);
      return a;
    }
    case "freebsd": {
      const a = ["freebsd", "-d"];
      if (opts.version) a.push("--version", opts.version);
      if (opts.firmware) a.push("--firmware", opts.firmware);
      if (opts.force) a.push("--force");
      a.push(...diskArgs(opts), ...netArgs(opts.net), ...nameArgs(opts), ...vmArgs(opts));
      return a;
    }
    case "netbsd": {
      const a = ["netbsd", "-d"];
      if (opts.version) a.push("--version", opts.version);
      if (opts.force) a.push("--force");
      a.push(...diskArgs(opts), ...netArgs(opts.net), ...nameArgs(opts), ...vmArgs(opts));
      return a;
    }
    case "firmware": {
      const a = [
        "firmware",
        "--firmware",
        opts.firmware,
        "--disk",
        opts.disk,
        "-d",
      ];
      a.push(...diskArgs(opts), ...netArgs(opts.net), ...nameArgs(opts), ...vmArgs(opts));
      return a;
    }
    case "kernel": {
      const a = ["kernel", "--kernel", opts.kernel, "-d"];
      if (opts.format) a.push("--format", opts.format);
      if (opts.initramfs) a.push("--initramfs", opts.initramfs);
      if (opts.cmdline) a.push("--cmdline", opts.cmdline);
      if (opts.disk) a.push("--disk", opts.disk);
      a.push(...diskArgs(opts), ...netArgs(opts.net), ...nameArgs(opts), ...vmArgs(opts));
      return a;
    }
  }
}
