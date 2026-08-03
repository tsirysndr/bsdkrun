"use strict";

// Maps the running platform/arch onto the bsdkrun host-binary release asset.
//
// Supported (mirrors the project's own platform support):
//   macOS  : arm64 only  (Apple Silicon / Hypervisor.framework)
//   Linux  : x64 + arm64 (KVM)
// Everything else — Windows, macOS on Intel, 32-bit, etc. — is unsupported.

const path = require("node:path");

// process.platform + process.arch  ->  release asset (a .tar.gz holding a single
// `bsdkrun` executable). Keep these names in lockstep with the `release`
// workflow's packaging step.
const ASSETS = {
  "darwin arm64": "bsdkrun-aarch64-apple-darwin.tar.gz",
  "linux x64": "bsdkrun-x86_64-unknown-linux-gnu.tar.gz",
  "linux arm64": "bsdkrun-aarch64-unknown-linux-gnu.tar.gz",
};

/** Human-readable list of what we do support, for error messages. */
const SUPPORTED = "macOS/arm64, Linux/x64, Linux/arm64";

/**
 * Resolve the release asset for the current platform, or throw a clear error
 * naming what's supported. Returns { platform, arch, asset }.
 */
function detect() {
  const platform = process.platform;
  const arch = process.arch;
  const key = platform + " " + arch;
  const asset = ASSETS[key];
  if (!asset) {
    throw new Error(
      `@bsdkrun/cli: unsupported platform ${platform}/${arch}.\n` +
        `bsdkrun ships prebuilt binaries only for: ${SUPPORTED}.\n` +
        (platform === "darwin"
          ? "macOS support is Apple Silicon (arm64) only — Intel Macs are not supported.\n"
          : "") +
        "If you're on a supported platform seeing this, please file an issue:\n" +
        "  https://github.com/tsirysndr/bsdkrun/issues"
    );
  }
  return { platform, arch, asset };
}

/** Absolute path to where the downloaded native binary lives inside the package. */
function binaryPath() {
  const name = process.platform === "win32" ? "bsdkrun.exe" : "bsdkrun";
  return path.join(__dirname, "..", "binaries", name);
}

module.exports = { detect, binaryPath, ASSETS, SUPPORTED };
