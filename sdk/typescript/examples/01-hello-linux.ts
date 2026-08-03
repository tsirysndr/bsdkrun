/**
 * The hello-world: boot an Alpine microVM, run a couple of commands, stop it.
 *
 *   bun  run examples/01-hello-linux.ts
 *   node examples/01-hello-linux.ts     # (after `bun run build`, import from dist)
 *   deno run -A examples/01-hello-linux.ts
 */
import { Sandbox } from "../src/index.ts";

const box = await Sandbox.create({ os: "linux", image: "alpine" });
console.log("booted sandbox", box.id);

// `sh` is a tagged template — interpolations are shell-quoted for you.
const who = await box.sh`whoami`.text();
console.log("running as:", who);

// `exec` takes argv directly (no shell parsing) — the safe default.
const uname = await box.exec(["uname", "-a"]);
console.log("kernel:", uname.text());

await box.stop();
console.log("stopped");
