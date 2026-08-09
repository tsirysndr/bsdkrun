// Plain JavaScript rather than TypeScript on purpose: `deno run` transpiles TS
// through a cache under DENO_DIR, and the unikernel's writable storage is a
// RAM filesystem that starts empty. Nothing here needs types.
Deno.serve({ port: 3000, hostname: "0.0.0.0" }, (req) => {
  const url = new URL(req.url);
  if (url.pathname === "/") {
    return new Response("Hello from Deno on Unikraft!\n");
  }
  if (url.pathname === "/info") {
    return Response.json({
      runtime: "deno",
      version: Deno.version.deno,
      v8: Deno.version.v8,
      typescript: Deno.version.typescript,
    });
  }
  return new Response("not found\n", { status: 404 });
});

console.log("Deno listening on port 3000");
