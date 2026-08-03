#!/usr/bin/env node
"use strict";

// Thin launcher: exec the platform-native `bsdkrun` binary that postinstall
// downloaded into ./binaries/, forwarding argv, stdio, and the exit status.

const fs = require("node:fs");
const { spawnSync } = require("node:child_process");
const { binaryPath, gvproxyPath } = require("../scripts/platform.js");

async function main() {
  const bin = binaryPath();

  // Self-heal: `bunx` (and any install with lifecycle scripts disabled) never
  // runs postinstall, so the binary won't be here. Download it on first use
  // rather than erroring out — this is the only run path under bunx.
  if (!fs.existsSync(bin)) {
    try {
      const { install } = require("../scripts/postinstall.js");
      await install();
    } catch (err) {
      console.error(
        "[bsdkrun] native binary not found and on-demand download failed:\n" +
          "  " +
          (err && err.message ? err.message : err) +
          "\nRun the install script manually:\n" +
          "  node " +
          require("node:path").join(__dirname, "..", "scripts", "postinstall.js")
      );
      process.exit(1);
    }
    if (!fs.existsSync(bin)) {
      console.error("[bsdkrun] native binary still missing after download at " + bin);
      process.exit(1);
    }
  }

  // Point bsdkrun at the gvproxy postinstall bundled, unless the user already
  // picked one (BSDKRUN_GVPROXY takes precedence in the native binary too, but
  // setting it here also wins over a different gvproxy on PATH).
  if (!process.env.BSDKRUN_GVPROXY) {
    const gv = gvproxyPath();
    if (fs.existsSync(gv)) {
      process.env.BSDKRUN_GVPROXY = gv;
    }
  }

  const res = spawnSync(bin, process.argv.slice(2), { stdio: "inherit" });

  if (res.error) {
    console.error("[bsdkrun] failed to launch: " + res.error.message);
    process.exit(1);
  }
  // Propagate a terminating signal as the conventional 128+signal exit code.
  if (res.signal) {
    process.exit(1);
  }
  process.exit(res.status === null ? 1 : res.status);
}

main();
