/**
 * Host-level inventory: list downloaded images, list + remove persistent
 * volumes, and run a toolchain probe. Mirrors `bsdkrun images` / `volume`.
 */
import { images, probe, volumes } from "../src/index.js";

// Verify libkrun links and a context can be configured (does not boot).
console.log("toolchain ok?", await probe());

// Downloaded images: pulled OCI images + fetched BSD images.
for (const im of await images.list()) {
  console.log(`image ${im.id}  ${im.reference}  ${im.size} bytes`);
}

// Persistent volumes.
const vols = await volumes.list();
for (const v of vols) {
  console.log(`volume ${v.name}  guest=${v.guest}  size=${v.size}`);
}

// Clean one up (force removes even if in use):
// await volumes.remove("demo-netbsd", { force: true });
