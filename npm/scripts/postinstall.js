"use strict";

// Runs after `npm install @bsdkrun/cli`. Downloads the prebuilt `bsdkrun` host
// binary for this platform from the matching GitHub release, verifies its
// SHA-256, and unpacks it into ./binaries/ where the bin shim expects it.
//
// Escape hatches (env vars):
//   BSDKRUN_SKIP_DOWNLOAD=1   skip everything (e.g. developing this package)
//   BSDKRUN_BINARY=/path      copy a local binary instead of downloading
//   BSDKRUN_DOWNLOAD_BASE=url  override the release download base URL

const fs = require("node:fs");
const path = require("node:path");
const crypto = require("node:crypto");
const { execFileSync } = require("node:child_process");
const { detect, binaryPath } = require("./platform.js");

const VERSION = require("../package.json").version;
const REPO = "tsirysndr/bsdkrun";
const DEFAULT_BASE = `https://github.com/${REPO}/releases/download/v${VERSION}`;

async function main() {
  if (process.env.BSDKRUN_SKIP_DOWNLOAD) {
    console.log("[bsdkrun] BSDKRUN_SKIP_DOWNLOAD set — skipping binary download.");
    return;
  }

  // detect() throws on unsupported platform/arch — surface that as a failed
  // install with a clear message (this is the real gate; `npm i -f` bypasses
  // the package.json os/cpu fields).
  const { asset } = detect();

  const dest = binaryPath();
  fs.mkdirSync(path.dirname(dest), { recursive: true });

  // Already installed (e.g. a rebuild in the same tree)? Nothing to do.
  if (fs.existsSync(dest) && !process.env.BSDKRUN_BINARY) {
    console.log(`[bsdkrun] binary already present at ${dest}`);
    return;
  }

  // Local binary override — copy it in, no network.
  if (process.env.BSDKRUN_BINARY) {
    fs.copyFileSync(process.env.BSDKRUN_BINARY, dest);
    fs.chmodSync(dest, 0o755);
    console.log(`[bsdkrun] installed from BSDKRUN_BINARY=${process.env.BSDKRUN_BINARY}`);
    return;
  }

  const base = process.env.BSDKRUN_DOWNLOAD_BASE || DEFAULT_BASE;
  const url = `${base}/${asset}`;

  console.log(`[bsdkrun] downloading ${url}`);
  const archive = await download(url);

  // Verify against the .sha256 sidecar the release publishes. If the sidecar is
  // missing (older releases), warn but continue; if present and mismatched, fail.
  await verifyChecksum(archive, `${url}.sha256`, asset);

  // Unpack the whole archive into the binaries dir. On macOS that's just
  // `bsdkrun`; on Linux it also carries the bundled libkrun/libkrunfw shared
  // objects, which must land next to the binary (its rpath is $ORIGIN).
  const destDir = path.dirname(dest);
  const tgz = path.join(destDir, asset);
  fs.writeFileSync(tgz, archive);
  try {
    // tar + gzip are present on every supported platform (macOS, Linux).
    execFileSync("tar", ["-xzf", tgz, "-C", destDir], { stdio: "inherit" });
  } finally {
    fs.rmSync(tgz, { force: true });
  }
  if (!fs.existsSync(dest)) {
    throw new Error(`archive ${asset} did not contain a 'bsdkrun' binary`);
  }
  fs.chmodSync(dest, 0o755);

  console.log(`[bsdkrun] installed bsdkrun ${VERSION} -> ${dest}`);
}

/** Fetch a URL to a Buffer, following redirects, with a clear error on failure. */
async function download(url) {
  const res = await fetch(url, { redirect: "follow" });
  if (!res.ok) {
    throw new Error(
      `failed to download ${url}: HTTP ${res.status} ${res.statusText}.\n` +
        `Is there a published release v${VERSION} with this asset?\n` +
        `  https://github.com/${REPO}/releases/tag/v${VERSION}`
    );
  }
  return Buffer.from(await res.arrayBuffer());
}

/** Verify `archive` against a `<asset>.sha256` sidecar (best-effort fetch, strict compare). */
async function verifyChecksum(archive, sidecarUrl, asset) {
  let res;
  try {
    res = await fetch(sidecarUrl, { redirect: "follow" });
  } catch {
    res = null;
  }
  if (!res || !res.ok) {
    console.warn(`[bsdkrun] no checksum sidecar for ${asset} — skipping verification.`);
    return;
  }
  // Sidecar is `shasum -a 256` output: "<hex>  <filename>". Take the first field.
  const expected = (await res.text()).trim().split(/\s+/)[0].toLowerCase();
  const actual = crypto.createHash("sha256").update(archive).digest("hex");
  if (expected !== actual) {
    throw new Error(
      `checksum mismatch for ${asset}:\n  expected ${expected}\n  actual   ${actual}`
    );
  }
  console.log(`[bsdkrun] checksum OK (${actual.slice(0, 12)}…)`);
}

main().catch((err) => {
  console.error("\n[bsdkrun] install failed:\n" + (err && err.message ? err.message : err));
  process.exit(1);
});
