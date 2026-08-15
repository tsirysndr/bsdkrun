/**
 * `exec` beyond the basics: env vars, stdin, working directory, exit codes,
 * and JSON output. Use `exec` (not `sh`) when you need these knobs or want to
 * avoid shell quoting entirely.
 */
import { Sandbox } from "../src/index.js";

const sbx = await Sandbox.create({ os: "linux", image: "alpine" });

try {
  // Environment variables.
  const env = await sbx.exec(["printenv", "GREETING"], {
    env: { GREETING: "hej" },
  });
  console.log("env GREETING =", env.text());

  // Pipe data to stdin.
  const wc = await sbx.exec(["wc", "-c"], { stdin: "12345" });
  console.log("byte count:", wc.text());

  // Run in a specific working directory.
  const pwd = await sbx.exec(["pwd"], { cwd: "/tmp" });
  console.log("cwd:", pwd.text());

  // Non-zero exit codes don't throw by default — inspect `.exitCode`.
  const bad = await sbx.exec(["false"]);
  console.log("`false` exited:", bad.exitCode, "ok?", bad.ok);

  // ...unless you opt in.
  try {
    await sbx.exec(["sh", "-c", "exit 3"], { throwOnError: true });
  } catch (err) {
    console.log("threw as expected:", (err as Error).message.split("\n")[0]);
  }

  // Vercel-Sandbox-style alias: program + args.
  const { stdout } = await sbx.runCommand("echo", ["from runCommand"]);
  console.log(stdout.trim());
} finally {
  await sbx.stop();
}
