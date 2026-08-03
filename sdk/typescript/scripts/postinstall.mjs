#!/usr/bin/env node
// Install-time provisioning for the bsdkrun CLI's native dependency, libkrun.
//
//   - macOS  → `brew install libkrun` (the libkrun.dylib the binary links).
//   - Linux  → download our PVH-enabled libkrun fork (needed for BSD PVH boots;
//              not packaged anywhere) into the bsdkrun cache. `libkrunfw` (the
//              bundled kernel lib it links) is a stock distro/nixpkgs dependency.
//
// Runs on `npm install` / `bun install`. Best-effort: it never fails the
// install (exit 0 always) — the SDK's runtime preflight retries if this was
// skipped (e.g. `--ignore-scripts`, or Deno, which doesn't run postinstall).
// Skip entirely with BSDKRUN_NO_AUTO_INSTALL=1.

import { spawnSync } from "node:child_process";
import { existsSync, mkdirSync } from "node:fs";
import { homedir } from "node:os";
import { join } from "node:path";
import process from "node:process";

const VERSION = process.env.BSDKRUN_LIBKRUN_VERSION || "v1.19.4-pvh";

function log(msg) {
  process.stderr.write(`[bsdkrun] ${msg}\n`);
}

function disabled() {
  const v = process.env.BSDKRUN_NO_AUTO_INSTALL;
  return v === "1" || v === "true";
}

function cacheRoot() {
  const base = process.env.XDG_CACHE_HOME || join(homedir(), ".cache");
  return join(base, "bsdkrun");
}

function sh(cmd, args) {
  return spawnSync(cmd, args, { encoding: "utf8" });
}

function provisionMacos() {
  const brewPaths = ["/opt/homebrew/bin/brew", "/usr/local/bin/brew"];
  let brew = brewPaths.find((p) => existsSync(p));
  if (!brew) {
    const r = sh("brew", ["--version"]);
    if (r.status === 0) brew = "brew";
  }
  if (!brew) {
    log(
      "Homebrew not found — install it (https://brew.sh) then " +
        "`brew install libkrun`. Skipping.",
    );
    return;
  }

  const prefix = sh(brew, ["--prefix", "libkrun"]);
  if (prefix.status === 0 && existsSync((prefix.stdout || "").trim())) {
    return; // already installed
  }

  log("installing libkrun via Homebrew (one-time)...");
  sh(brew, ["tap", "libkrun/krun"]);
  const inst = spawnSync(brew, ["install", "libkrun"], { stdio: "inherit" });
  if (inst.status !== 0) {
    log("`brew install libkrun` failed — install it manually: brew install libkrun");
    return;
  }
  log("libkrun installed.");
}

function archSlug() {
  if (process.arch === "x64") return "x86_64";
  if (process.arch === "arm64") return "aarch64";
  return undefined;
}

function provisionLinux() {
  if (process.env.BSDKRUN_LIBKRUN_DIR) return; // caller manages it
  const slug = archSlug();
  if (!slug) {
    log(`unsupported arch ${process.arch} — skipping libkrun download.`);
    return;
  }

  const dir = join(cacheRoot(), "libkrun-pvh", VERSION);
  const marker = join(dir, "lib64", "libkrun.so");
  if (existsSync(marker)) return; // already downloaded

  const asset = `libkrun-pvh-${VERSION}-${slug}-unknown-linux-gnu.tar.gz`;
  const url = `https://github.com/tsirysndr/libkrun/releases/download/${VERSION}/${asset}`;

  log(`downloading PVH libkrun ${VERSION} (${slug})...`);
  mkdirSync(dir, { recursive: true });
  const r = sh("/bin/sh", [
    "-c",
    `curl -fsSL ${JSON.stringify(url)} | tar xz -C ${JSON.stringify(dir)}`,
  ]);
  if (r.status !== 0 || !existsSync(marker)) {
    log(
      `failed to download PVH libkrun (${r.stderr || r.stdout || "unknown"}). ` +
        `Fetch it manually from ${url} and set BSDKRUN_LIBKRUN_DIR.`,
    );
    return;
  }
  log(`PVH libkrun ready at ${join(dir, "lib64")}`);
}

function main() {
  if (disabled()) return;
  try {
    if (process.platform === "darwin") provisionMacos();
    else if (process.platform === "linux") provisionLinux();
  } catch (err) {
    log(`provisioning skipped: ${err?.message ?? err}`);
  }
}

main();
// Never fail the install — the runtime preflight is the safety net.
process.exit(0);
