/**
 * The `sh` tagged template in depth — quoting, chaining, JSON, `raw`, and
 * `.nothrow()`. Great for quick shell one-liners inside the guest.
 */
import { raw, Sandbox } from "../src/index.ts";

const box = await Sandbox.create({ os: "linux", image: "alpine" });

try {
  // Interpolations are single-quoted, so this is injection-safe.
  const dir = "/etc; rm -rf /"; // hostile input — harmless here
  const listing = await box.sh`ls ${dir} 2>&1 || echo "no such dir"`.text();
  console.log("safe listing:", listing);

  // `.text()`, `.json()`, `.lines()` are convenience accessors.
  await box.exec(["apk", "add", "--no-cache", "jq"]).catch(() => {});
  const os = await box.sh`cat /etc/os-release | head -2`.lines();
  console.log("os lines:", os);

  // `.nothrow()` keeps a non-zero exit from throwing.
  const missing = await box.sh`cat /nope`.nothrow();
  console.log("missing exit:", missing.exitCode);

  // `.env()` sets variables just for this command.
  const greet = await box.sh`echo "$GREETING world"`.env({ GREETING: "hello" }).text();
  console.log(greet);

  // `raw()` splices a value in WITHOUT quoting (trusted content only).
  const flags = raw("-la /var");
  const ll = await box.sh`ls ${flags} | head -3`.text();
  console.log(ll);
} finally {
  await box.stop();
}
