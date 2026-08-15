/**
 * Browser terminal over xterm.js.
 *
 * Boots an Alpine sandbox, then serves a tiny web page whose xterm.js terminal
 * is bridged to a real PTY inside the guest over a WebSocket. Runs on Bun (uses
 * Bun.serve for both the static page and the WS upgrade).
 *
 *   bun run examples/08-browser-terminal/server.ts
 *   # then open http://localhost:3000
 *
 * The SDK's `Terminal` speaks the guest agent's TCP protocol directly, so
 * keystrokes, output, and live window-resize all flow end to end — no CLI PTY,
 * no host TTY required.
 */
import { readFileSync } from "node:fs";
import { join } from "node:path";
import { Sandbox } from "../../src/index.js";
import type { Terminal } from "../../src/index.js";

const html = readFileSync(join(import.meta.dir, "index.html"), "utf8");

console.log("booting sandbox...");
const sbx = await Sandbox.create({
  os: "linux",
  image: "alpine",
  command: ["sleep", "86400"],
});
console.log("sandbox", sbx.id, "ready");

// Wait for the in-guest agent to answer before accepting terminals.
for (let i = 0; i < 20; i++) {
  if ((await sbx.exec(["true"])).exitCode === 0) break;
  await new Promise((r) => setTimeout(r, 1500));
}

interface WsData {
  term: Terminal | null;
}

const server = Bun.serve<WsData>({
  port: 3000,
  async fetch(req, srv) {
    const url = new URL(req.url);
    if (url.pathname === "/ws") {
      // Attach a mutable holder; `open` fills in the terminal.
      if (srv.upgrade(req, { data: { term: null } })) return;
      return new Response("upgrade failed", { status: 400 });
    }
    return new Response(html, { headers: { "content-type": "text/html" } });
  },
  websocket: {
    async open(ws) {
      const term = await sbx.terminal({ command: ["/bin/sh"], cols: 100, rows: 30 });
      ws.data.term = term;
      // Guest output -> browser.
      term.onData((chunk) => ws.send(chunk.toString("utf8")));
      term.onExit(() => ws.close());
    },
    message(ws, raw) {
      const term = ws.data.term;
      if (!term) return;
      const text = typeof raw === "string" ? raw : raw.toString();
      // A JSON control frame carries resizes; anything else is keystrokes.
      if (text.startsWith("{") && text.includes("resize")) {
        try {
          const { resize } = JSON.parse(text) as { resize?: [number, number] };
          if (resize) return term.resize(resize[0], resize[1]);
        } catch {
          /* fall through as data */
        }
      }
      term.write(text);
    },
    close(ws) {
      ws.data.term?.kill();
    },
  },
});

console.log(`\n  open http://localhost:${server.port}\n`);

process.on("SIGINT", async () => {
  console.log("\nstopping sandbox...");
  await sbx.stop().catch(() => {});
  process.exit(0);
});
