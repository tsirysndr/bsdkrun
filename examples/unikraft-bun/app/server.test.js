// A smoke test of the real thing: the server listens on import, so the test
// runs it as a child — the same binary boundary the unikernel e2e asserts.
import { test, expect, afterAll } from "bun:test";

const child = Bun.spawn(["bun", "app/server.js"], { stdout: "ignore" });
afterAll(() => child.kill());

async function ready(url, tries = 50) {
  for (let i = 0; i < tries; i++) {
    try {
      return await fetch(url);
    } catch {
      await Bun.sleep(100);
    }
  }
  throw new Error("server never came up");
}

test("/ greets", async () => {
  const res = await ready("http://127.0.0.1:3000/");
  expect(await res.text()).toBe("Hello from Bun on Unikraft!\n");
});

test("/info reports the runtime", async () => {
  const res = await fetch("http://127.0.0.1:3000/info");
  const body = await res.json();
  expect(body.runtime).toBe("bun");
});
