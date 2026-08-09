// Bun's HTTP server. `Bun.serve` returns once the listener is up; the runtime
// keeps the process alive, so there is nothing to await here.
const server = Bun.serve({
  port: 3000,
  hostname: "0.0.0.0",
  fetch(req) {
    const url = new URL(req.url);
    if (url.pathname === "/") {
      return new Response("Hello from Bun on Unikraft!\n");
    }
    if (url.pathname === "/info") {
      return Response.json({
        runtime: "bun",
        version: Bun.version,
        revision: Bun.revision.slice(0, 12),
      });
    }
    return new Response("not found\n", { status: 404 });
  },
});

console.log(`Bun listening on port ${server.port}`);
