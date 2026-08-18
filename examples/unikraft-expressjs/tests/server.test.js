// A smoke test of the real thing: the app listens on import, so the test
// runs it as a child — the same binary boundary the unikernel e2e asserts.
const { test, after } = require("node:test");
const assert = require("node:assert");
const { spawn } = require("node:child_process");

const child = spawn(process.execPath, ["app/index.js"], { stdio: "ignore" });
after(() => child.kill());

async function ready(url, tries = 50) {
  for (let i = 0; i < tries; i++) {
    try {
      return await fetch(url);
    } catch {
      await new Promise((r) => setTimeout(r, 100));
    }
  }
  throw new Error("server never came up");
}

test("/ greets", async () => {
  const res = await ready("http://127.0.0.1:3000/");
  assert.strictEqual(await res.text(), "Bye, World!\n");
});
