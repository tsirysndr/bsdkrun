import { describe, expect, test } from "bun:test";
import { buildCreateArgs } from "../src/args.js";
import { buildScript, CommandResult, raw } from "../src/shell.js";
import { CommandFailedError } from "../src/errors.js";
import { libkrunTarballUrl, linuxArchSlug } from "../src/preflight.js";

describe("sh template quoting", () => {
  const build = (s: TemplateStringsArray, ...v: unknown[]) => buildScript(s, v);

  test("plain interpolation is single-quoted", () => {
    expect(build`echo ${"hello"}`).toBe("echo 'hello'");
  });

  test("injection attempts stay quoted", () => {
    expect(build`ls ${"/etc; rm -rf /"}`).toBe("ls '/etc; rm -rf /'");
  });

  test("embedded single quotes are escaped", () => {
    expect(build`echo ${"it's"}`).toBe(`echo 'it'\\''s'`);
  });

  test("numbers and booleans are unquoted", () => {
    expect(build`sleep ${5}`).toBe("sleep 5");
    expect(build`x ${true}`).toBe("x true");
  });

  test("arrays are space-joined and each element quoted", () => {
    expect(build`cmd ${["a", "b c"]}`).toBe("cmd 'a' 'b c'");
  });

  test("raw() splices verbatim", () => {
    expect(build`ls ${raw("-la /var")}`).toBe("ls -la /var");
  });

  test("null / undefined become empty quotes", () => {
    expect(build`echo ${null}`).toBe("echo ''");
    expect(build`echo ${undefined}`).toBe("echo ''");
  });
});

describe("CommandResult", () => {
  test("text() trims trailing newlines", () => {
    expect(new CommandResult("hi\n\n", "", 0, "x").text()).toBe("hi");
  });

  test("json() parses stdout", () => {
    expect(new CommandResult('{"a":1}', "", 0, "x").json<{ a: number }>()).toEqual({
      a: 1,
    });
  });

  test("lines() drops empty lines", () => {
    expect(new CommandResult("a\n\nb\n", "", 0, "x").lines()).toEqual(["a", "b"]);
  });

  test("ok reflects the exit code", () => {
    expect(new CommandResult("", "", 0, "x").ok).toBe(true);
    expect(new CommandResult("", "", 1, "x").ok).toBe(false);
  });

  test("throwIfFailed throws on non-zero", () => {
    expect(() => new CommandResult("", "boom", 2, "x").throwIfFailed()).toThrow(
      CommandFailedError,
    );
    expect(new CommandResult("", "", 0, "x").throwIfFailed().exitCode).toBe(0);
  });
});

describe("preflight helpers", () => {
  test("linuxArchSlug maps node arch to release triples", () => {
    expect(linuxArchSlug("x64")).toBe("x86_64");
    expect(linuxArchSlug("arm64")).toBe("aarch64");
    expect(linuxArchSlug("ppc64")).toBeUndefined();
  });

  test("libkrunTarballUrl builds the fork release URL", () => {
    expect(libkrunTarballUrl("x86_64", "v1.19.4-pvh")).toBe(
      "https://github.com/tsirysndr/libkrun/releases/download/" +
        "v1.19.4-pvh/libkrun-pvh-v1.19.4-pvh-x86_64-unknown-linux-gnu.tar.gz",
    );
    expect(libkrunTarballUrl("aarch64", "v1.19.4-pvh")).toContain(
      "aarch64-unknown-linux-gnu.tar.gz",
    );
  });
});

describe("buildCreateArgs", () => {
  test("linux: image, detach, resources, ports, command", () => {
    const args = buildCreateArgs({
      os: "linux",
      image: "alpine",
      cpus: 2,
      mem: 1024,
      net: { ports: ["8080:80", { host: 2222, guest: 22 }] },
      command: ["sleep", "300"],
    });
    expect(args).toEqual([
      "linux", "alpine", "-d",
      "--port", "8080:80", "--port", "2222:22",
      "--cpus", "2", "--mem", "1024",
      "--", "sleep", "300",
    ]);
  });

  test("linux: volume, mounts, initramfs, no-net", () => {
    const args = buildCreateArgs({
      os: "linux",
      image: "debian",
      volume: "web",
      mounts: ["~/p:/src", "~/d:/data:ro"],
      initramfs: true,
      net: { disabled: true },
    });
    expect(args).toContain("-v");
    expect(args).toContain("web");
    expect(args).toContain("--initramfs");
    expect(args).toContain("--no-net");
    expect(args.filter((a) => a === "--mount")).toHaveLength(2);
  });

  test("freebsd: version + persist + attach-disk", () => {
    const args = buildCreateArgs({
      os: "freebsd",
      version: "14.3",
      persist: true,
      attachDisk: ["extra.raw:ro"],
    });
    expect(args.slice(0, 2)).toEqual(["freebsd", "-d"]);
    expect(args).toContain("--version");
    expect(args).toContain("14.3");
    expect(args).toContain("--persist");
    expect(args).toContain("--attach-disk");
    expect(args).toContain("extra.raw:ro");
  });

  test("netbsd: volume + force", () => {
    const args = buildCreateArgs({ os: "netbsd", volume: "db", force: true });
    expect(args.slice(0, 2)).toEqual(["netbsd", "-d"]);
    expect(args).toContain("-v");
    expect(args).toContain("db");
    expect(args).toContain("--force");
  });

  test("firmware: firmware + disk required", () => {
    const args = buildCreateArgs({
      os: "firmware",
      firmware: "KRUN_EFI.fd",
      disk: "disk.raw",
    });
    expect(args).toEqual([
      "firmware", "--firmware", "KRUN_EFI.fd", "--disk", "disk.raw", "-d",
    ]);
  });

  test("network + name: joins a global network with a DNS name", () => {
    const args = buildCreateArgs({
      os: "linux",
      image: "postgres",
      name: "db",
      net: { network: "devnet" },
    });
    expect(args).toContain("--network");
    expect(args).toContain("devnet");
    expect(args).toContain("--name");
    expect(args).toContain("db");
    // order: --network comes from netArgs, --name after it
    expect(args.indexOf("--name")).toBeGreaterThan(args.indexOf("--network"));
  });

  test("name works across guest kinds (freebsd)", () => {
    const args = buildCreateArgs({ os: "freebsd", name: "bsd", net: { network: "n" } });
    expect(args).toContain("--name");
    expect(args).toContain("bsd");
    expect(args).toContain("--network");
  });

  test("kernel: format, cmdline, disk", () => {
    const args = buildCreateArgs({
      os: "kernel",
      kernel: "netbsd",
      format: "raw",
      cmdline: "console=com",
      disk: "root.raw",
    });
    expect(args.slice(0, 4)).toEqual(["kernel", "--kernel", "netbsd", "-d"]);
    expect(args).toContain("--format");
    expect(args).toContain("raw");
    expect(args).toContain("--cmdline");
    expect(args).toContain("console=com");
    expect(args).toContain("--disk");
    expect(args).toContain("root.raw");
  });
});

describe("buildCreateArgs env", () => {
  // Object key order is not something a caller should have to think about, so
  // the builder sorts — otherwise the same options could produce a different
  // command line.
  test("emits -e K=V sorted by key", () => {
    expect(
      buildCreateArgs({
        os: "linux",
        image: "alpine",
        env: { ZED: "3", ALPHA: "1", MID: "2" },
      }),
    ).toEqual(["linux", "alpine", "-d", "-e", "ALPHA=1", "-e", "MID=2", "-e", "ZED=3"]);
  });

  test("emits nothing without env", () => {
    expect(buildCreateArgs({ os: "linux", image: "alpine" })).toEqual([
      "linux",
      "alpine",
      "-d",
    ]);
    expect(buildCreateArgs({ os: "linux", image: "alpine", env: {} })).toEqual([
      "linux",
      "alpine",
      "-d",
    ]);
  });
});
