// A smoke test of the real thing: the server listens on import, so the test
// runs it as a child — the same binary boundary the unikernel e2e asserts.
const child = new Deno.Command("deno", {
  args: ["run", "--quiet", "--allow-net", "app/server.js"],
  stdout: "null",
}).spawn();

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

Deno.test("/ greets and /info reports the runtime", async () => {
  const res = await ready("http://127.0.0.1:3000/");
  if ((await res.text()) !== "Hello from Deno on Unikraft!\n") {
    throw new Error("greeting body wrong");
  }
  const info = await (await fetch("http://127.0.0.1:3000/info")).json();
  if (info.runtime !== "deno") throw new Error("info.runtime wrong");
  child.kill();
  await child.status;
});
