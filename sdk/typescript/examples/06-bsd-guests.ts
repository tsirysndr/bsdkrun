/**
 * BSD guests. `netbsd` direct-boots its kernel everywhere (macOS + Linux);
 * `freebsd` boots via EFI on macOS and PVH on Linux/amd64. Both carry the same
 * machine options and — on bsdkrun's bundled images — the exec agent.
 */
import { Sandbox } from "../src/index.js";

// NetBSD current, in the background, with a persistent volume.
const nb = await Sandbox.create({
  os: "netbsd",
  volume: "demo-netbsd",
  net: { ports: ["2223:22"] },
});
console.log("NetBSD sandbox:", nb.id);

// The bundled NetBSD image bakes in the agent, so exec works once it's up.
// (A microVM boots in seconds; poll rather than a fixed sleep in real code.)
try {
  const uname = await nb.exec(["uname", "-a"]);
  console.log("guest:", uname.text());
} catch {
  console.log("agent not up yet — check `logs`");
} finally {
  await nb.stop();
}

// FreeBSD is the same shape:
//   const fb = await Sandbox.create({ os: "freebsd", version: "14.3" });
