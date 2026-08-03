/**
 * Machine lifecycle: create detached, list, reconnect by id, read the console
 * log, inspect status, and stop. Mirrors `bsdkrun ps` / `logs` / `stop`.
 */
import { Sandbox } from "../src/index.ts";

// Give it a long-running command so it stays up while we poke at it.
const box = await Sandbox.create({
  os: "linux",
  image: "alpine",
  command: ["sleep", "300"],
});
console.log("created", box.id);

// Reconnect from just an id (a unique prefix is enough) — e.g. in another
// process or a later run.
const again = await Sandbox.get(box.id.slice(0, 6));
console.log("reconnected via prefix:", again.id);

// Enumerate running machines.
const running = await Sandbox.list();
console.log(
  "running machines:",
  running.map((m) => `${m.id}(${m.kind})`).join(", "),
);

// Full status row.
const info = await box.status();
console.log("status:", info?.status, "cpus:", info?.cpus, "mem:", info?.mem);

// The console log so far.
const log = await box.logs();
console.log("console log bytes:", log.length);

await box.stop();
console.log("stopped; still running?", await box.isRunning());
