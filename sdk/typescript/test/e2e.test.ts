/**
 * End-to-end tests: these boot a real Alpine microVM through the `bsdkrun`
 * binary, so they need libkrun + KVM (Linux) or Hypervisor.framework (macOS).
 *
 * They run only when a `bsdkrun` binary is discoverable AND `BSDKRUN_E2E=1` is
 * set (CI sets it after building the binary), so a plain `bun test` on a dev
 * box without the toolchain just runs the unit suite.
 */
import { afterAll, beforeAll, describe, expect, test } from "bun:test";
import { Sandbox } from "../src/sandbox.ts";
import { images } from "../src/images.ts";
import { volumes } from "../src/volumes.ts";
import { probe } from "../src/system.ts";

const ENABLED = process.env.BSDKRUN_E2E === "1";
const IMAGE = process.env.BSDKRUN_E2E_IMAGE ?? "alpine";
const d = ENABLED ? describe : describe.skip;

d("e2e: bsdkrun SDK against a live guest", () => {
  let box: Sandbox;

  beforeAll(async () => {
    box = await Sandbox.create({
      os: "linux",
      image: IMAGE,
      command: ["sleep", "600"],
    });
    // Give the in-guest agent a moment to come up.
    for (let i = 0; i < 20; i++) {
      const r = await box.exec(["true"]);
      if (r.exitCode === 0) return;
      await new Promise((res) => setTimeout(res, 1500));
    }
  });

  afterAll(async () => {
    if (box) await box.stop().catch(() => {});
  });

  test("probe reports a working toolchain", async () => {
    expect(await probe()).toBe(true);
  });

  test("create returned a valid id and the machine is running", async () => {
    expect(box.id).toMatch(/^[0-9a-f]{12}$/);
    expect(await box.isRunning()).toBe(true);
  });

  test("exec runs argv and captures stdout + exit code", async () => {
    const r = await box.exec(["echo", "hello-e2e"]);
    expect(r.exitCode).toBe(0);
    expect(r.text()).toBe("hello-e2e");
    expect(r.ok).toBe(true);
  });

  test("exec forwards env vars", async () => {
    const r = await box.exec(["printenv", "GREETING"], {
      env: { GREETING: "hej" },
    });
    expect(r.text()).toBe("hej");
  });

  test("exec pipes stdin", async () => {
    const r = await box.exec(["wc", "-c"], { stdin: "12345" });
    expect(r.text().trim()).toBe("5");
  });

  test("exec honors cwd", async () => {
    const r = await box.exec(["pwd"], { cwd: "/tmp" });
    expect(r.text()).toBe("/tmp");
  });

  test("exec surfaces non-zero exit without throwing", async () => {
    const r = await box.exec(["sh", "-c", "exit 7"]);
    expect(r.exitCode).toBe(7);
    expect(r.ok).toBe(false);
  });

  test("exec throwOnError throws CommandFailedError", async () => {
    await expect(
      box.exec(["sh", "-c", "exit 2"], { throwOnError: true }),
    ).rejects.toThrow();
  });

  test("runCommand alias works", async () => {
    const r = await box.runCommand("uname", ["-s"]);
    expect(r.text()).toBe("Linux");
  });

  test("sh template runs and quotes interpolations", async () => {
    const r = await box.sh`echo ${"a b c"}`;
    expect(r.text()).toBe("a b c");
  });

  test("sh .nothrow() keeps a non-zero exit from throwing", async () => {
    const r = await box.sh`cat /definitely/not/here`.nothrow();
    expect(r.exitCode).not.toBe(0);
  });

  test("sh .env() sets a variable for the command", async () => {
    const r = await box.sh`echo "$X"`.env({ X: "1" });
    expect(r.text()).toBe("1");
  });

  test("logs returns the console log", async () => {
    const log = await box.logs();
    expect(typeof log).toBe("string");
  });

  test("list includes the running machine", async () => {
    const running = await Sandbox.list();
    expect(running.some((m) => m.id === box.id)).toBe(true);
  });

  test("get reconnects by id prefix", async () => {
    const again = await Sandbox.get(box.id.slice(0, 6));
    expect(again.id).toBe(box.id);
  });

  test("status returns a typed row", async () => {
    const info = await box.status();
    expect(info?.id).toBe(box.id);
    expect(info?.kind).toBe("linux");
    expect(info?.running).toBe(true);
  });

  test("images.list returns typed rows", async () => {
    const list = await images.list();
    expect(Array.isArray(list)).toBe(true);
    if (list.length) {
      expect(typeof list[0]!.reference).toBe("string");
      expect(typeof list[0]!.size).toBe("number");
    }
  });

  test("volumes.list returns typed rows", async () => {
    const list = await volumes.list();
    expect(Array.isArray(list)).toBe(true);
  });

  test("stop terminates the machine", async () => {
    await box.stop();
    // give ps a moment to reconcile
    await new Promise((res) => setTimeout(res, 1000));
    expect(await box.isRunning()).toBe(false);
  });
});
