/**
 * Networking: forward host ports into the guest and set up key-based SSH via
 * the in-guest agent. After `ssh setup` you can really `ssh -p <port> root@…`.
 */
import { Sandbox } from "../src/index.js";

const sbx = await Sandbox.create({
  os: "linux",
  image: "alpine",
  command: ["sleep", "300"],
  net: {
    // Forward host 2222 -> guest 22. Accepts "2222:22" or {host, guest}.
    ports: [{ host: 2222, guest: 22 }],
  },
});

try {
  // Alpine ships root locked; unlock to key-only login for the demo.
  await sbx.exec(["sh", "-c", "sed -i 's/^root:!:/root::/' /etc/shadow"]);

  // Install your local ~/.ssh/id_*.pub keys + sshd, enable + start it.
  const setup = await sbx.ssh.setup();
  console.log(setup.text());

  const status = await sbx.ssh.status();
  console.log("sshd status:", status.text());

  console.log("now: ssh -p 2222 root@127.0.0.1");
} finally {
  await sbx.stop();
}
