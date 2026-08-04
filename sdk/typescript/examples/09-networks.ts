/**
 * Global networks: put two machines on a shared subnet so they reach each other
 * by name (docker-compose style), with internal DNS. Mirrors `bsdkrun network`.
 */
import { Sandbox, networks } from "../src/index.js";

// A shared network (starts its gvproxy switch).
await networks.create("devnet");

// Two members with DNS names. Each gets a distinct IP on 192.168.127.0/24.
const db = await Sandbox.create({
  os: "linux",
  image: "alpine",
  name: "db",
  net: { network: "devnet" },
  command: ["sleep", "600"],
});
const api = await Sandbox.create({
  os: "linux",
  image: "alpine",
  name: "api",
  net: { network: "devnet" },
  command: ["sleep", "600"],
});

// api reaches db by name.
console.log(await api.sh`ping -c1 db`.text());

// Inspect membership.
for (const n of await networks.list()) {
  console.log(`network ${n.name}  ${n.subnet}  ${n.running}/${n.members} up`);
}
for (const m of await networks.members("devnet")) {
  console.log(`  member ${m.name ?? m.id}  ip=${m.netIp}`);
}

// Edit membership (applies on the next start).
await api.disconnectNetwork();
await api.start();

// Refresh /etc/hosts if a member can't resolve peers (notably NetBSD).
await networks.sync("devnet");

// Cleanup.
await db.remove({ force: true });
await api.remove({ force: true });
await networks.remove("devnet", { force: true });
