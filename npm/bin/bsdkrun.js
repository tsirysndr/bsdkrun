#!/usr/bin/env node
"use strict";

// Thin launcher: exec the platform-native `bsdkrun` binary that postinstall
// downloaded into ./binaries/, forwarding argv, stdio, and the exit status.

const fs = require("node:fs");
const { spawnSync } = require("node:child_process");
const { binaryPath } = require("../scripts/platform.js");

const bin = binaryPath();

if (!fs.existsSync(bin)) {
  console.error(
    "[bsdkrun] native binary not found at " +
      bin +
      "\nThe postinstall download may have been skipped (e.g. `npm install --ignore-scripts`).\n" +
      "Reinstall @bsdkrun/cli, or run its install script manually:\n" +
      "  node " +
      require("node:path").join(__dirname, "..", "scripts", "postinstall.js")
  );
  process.exit(1);
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
