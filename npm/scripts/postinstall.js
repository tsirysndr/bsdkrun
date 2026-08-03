"use strict";

// Runs after `npm install @bsdkrun/cli`. Downloads the prebuilt `bsdkrun` host
// binary for this platform from the matching GitHub release, verifies its
// SHA-256, and unpacks it into ./binaries/ where the bin shim expects it.
//
// Escape hatches (env vars):
//   BSDKRUN_SKIP_DOWNLOAD=1     skip everything (e.g. developing this package)
//   BSDKRUN_BINARY=/path        copy a local binary instead of downloading
//   BSDKRUN_DOWNLOAD_BASE=url    override the release download base URL
//   BSDKRUN_SKIP_GVPROXY=1      skip the gvproxy (user-mode networking) download
//   BSDKRUN_GVPROXY=/path       already have gvproxy — skip its download
//   BSDKRUN_GVPROXY_VERSION=vX  pin a gvproxy release tag (default: latest)

const fs = require("node:fs");
const path = require("node:path");
const crypto = require("node:crypto");
const { execFileSync } = require("node:child_process");
const { detect, binaryPath, gvproxyAsset, gvproxyPath } = require("./platform.js");

const VERSION = require("../package.json").version;
const REPO = "tsirysndr/bsdkrun";
const DEFAULT_BASE = `https://github.com/${REPO}/releases/download/v${VERSION}`;

// gvproxy (gvisor-tap-vsock) provides the guest's user-mode networking. bsdkrun
// boots fine without it (just no NIC), so this download is best-effort.
const GVPROXY_REPO = "containers/gvisor-tap-vsock";

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

  // Best-effort: also fetch gvproxy for user-mode guest networking. Never fail
  // the install over it — bsdkrun degrades gracefully to a NIC-less boot.
  await installGvproxy();
}

/**
 * Download the gvproxy binary for this platform into ./binaries/gvproxy so guest
 * networking works out of the box. Best-effort: any failure just warns, since
 * the guest can still boot without a NIC and users can `brew install gvproxy`
 * or set BSDKRUN_GVPROXY themselves.
 */
async function installGvproxy() {
  if (process.env.BSDKRUN_SKIP_GVPROXY) {
    console.log("[bsdkrun] BSDKRUN_SKIP_GVPROXY set — skipping gvproxy download.");
    return;
  }
  if (process.env.BSDKRUN_GVPROXY) {
    console.log(
      `[bsdkrun] BSDKRUN_GVPROXY=${process.env.BSDKRUN_GVPROXY} set — skipping gvproxy download.`
    );
    return;
  }

  const asset = gvproxyAsset();
  if (!asset) return; // unsupported platform already handled by detect()

  const dest = gvproxyPath();
  if (fs.existsSync(dest)) {
    console.log(`[bsdkrun] gvproxy already present at ${dest}`);
    return;
  }

  const version = process.env.BSDKRUN_GVPROXY_VERSION;
  const base = version
    ? `https://github.com/${GVPROXY_REPO}/releases/download/${version}`
    : `https://github.com/${GVPROXY_REPO}/releases/latest/download`;
  const url = `${base}/${asset}`;

  try {
    console.log(`[bsdkrun] downloading ${url}`);
    const bin = await download(url);
    // gvproxy assets are raw executables (no archive) — write and mark runnable.
    fs.mkdirSync(path.dirname(dest), { recursive: true });
    fs.writeFileSync(dest, bin);
    fs.chmodSync(dest, 0o755);
    console.log(`[bsdkrun] installed gvproxy -> ${dest}`);
  } catch (err) {
    console.warn(
      `[bsdkrun] could not download gvproxy (${err && err.message ? err.message : err}).\n` +
        "         Guest networking will be unavailable until you install gvproxy\n" +
        "         (`brew install gvproxy`) or set BSDKRUN_GVPROXY to its path."
    );
  }
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

// Exported so the bin shim can self-heal: under `bunx` (and any install run
// with lifecycle scripts disabled) postinstall never runs, so the launcher
// calls install() on first use to fetch the binary on demand.
module.exports = { install: main };

// Only auto-run when executed directly (`node scripts/postinstall.js`), not
// when require()'d by the bin shim.
if (require.main === module) {
  main().catch((err) => {
    console.error("\n[bsdkrun] install failed:\n" + (err && err.message ? err.message : err));
    process.exit(1);
  });
}
