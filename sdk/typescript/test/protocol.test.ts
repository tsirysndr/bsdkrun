import { afterEach, describe, expect, test } from "bun:test";
import { createServer, type Server, type Socket } from "node:net";
import { AgentConnection } from "../src/agent-protocol.js";

/**
 * Exercise the framed-protocol client against a local fake agent — no guest
 * needed. The fake reads the request header, then speaks frames back so we can
 * assert the client parses split/coalesced frames and decodes exit codes.
 */

const CH = { STDIN: 0, STDOUT: 1, STDERR: 2, EXIT: 3, WINSZ: 4 };

function frame(chan: number, payload: Buffer): Buffer {
  const hdr = Buffer.allocUnsafe(5);
  hdr[0] = chan;
  hdr.writeUInt32LE(payload.length, 1);
  return Buffer.concat([hdr, payload]);
}

/** Parse the client's request header: tty flag + argv + env. */
function parseRequest(buf: Buffer): { tty: number; argv: string[]; env: string[] } {
  let off = 0;
  const tty = buf[off++]!;
  const readVec = () => {
    const n = buf.readUInt32LE(off);
    off += 4;
    const out: string[] = [];
    for (let i = 0; i < n; i++) {
      const len = buf.readUInt32LE(off);
      off += 4;
      out.push(buf.subarray(off, off + len).toString("utf8"));
      off += len;
    }
    return out;
  };
  const argv = readVec();
  const env = readVec();
  return { tty, argv, env };
}

let server: Server | undefined;

afterEach(() => {
  server?.close();
  server = undefined;
});

function listen(onConn: (sock: Socket) => void): Promise<number> {
  return new Promise((resolve) => {
    server = createServer(onConn);
    server.listen(0, "127.0.0.1", () => {
      const addr = server!.address();
      resolve(typeof addr === "object" && addr ? addr.port : 0);
    });
  });
}

describe("AgentConnection wire protocol", () => {
  test("sends a well-formed request and parses stdout + exit", async () => {
    let seen: ReturnType<typeof parseRequest> | undefined;
    const port = await listen((sock) => {
      sock.once("data", (buf: Buffer) => {
        seen = parseRequest(buf);
        // Reply with two stdout chunks (split across writes) then exit 0.
        sock.write(frame(CH.STDOUT, Buffer.from("hello ")));
        sock.write(frame(CH.STDOUT, Buffer.from("world")));
        sock.write(frame(CH.EXIT, (() => {
          const b = Buffer.allocUnsafe(4);
          b.writeUInt32LE(0, 0);
          return b;
        })()));
      });
    });

    let out = "";
    const code = await new Promise<number>(async (resolve) => {
      await AgentConnection.connect(
        "127.0.0.1",
        port,
        { argv: ["/bin/echo", "hi"], env: ["A=1"], tty: false },
        {
          onStdout: (d) => {
            out += d.toString("utf8");
          },
          onExit: resolve,
        },
      );
    });

    expect(code).toBe(0);
    expect(out).toBe("hello world");
    expect(seen?.tty).toBe(0);
    expect(seen?.argv).toEqual(["/bin/echo", "hi"]);
    expect(seen?.env).toEqual(["A=1"]);
  });

  test("coalesces a frame split across two TCP packets", async () => {
    const port = await listen((sock) => {
      sock.once("data", () => {
        const f = frame(CH.STDOUT, Buffer.from("SPLIT"));
        sock.write(f.subarray(0, 3)); // partial header+payload
        setTimeout(() => {
          sock.write(f.subarray(3));
          sock.write(frame(CH.EXIT, Buffer.from([5, 0, 0, 0])));
        }, 20);
      });
    });

    let out = "";
    const code = await new Promise<number>(async (resolve) => {
      await AgentConnection.connect(
        "127.0.0.1",
        port,
        { argv: ["x"], tty: false },
        { onStdout: (d) => (out += d.toString("utf8")), onExit: resolve },
      );
    });
    expect(out).toBe("SPLIT");
    expect(code).toBe(5);
  });

  test("encodes a tty request with an initial winsz frame", async () => {
    let firstFrame: Buffer | undefined;
    const port = await listen((sock) => {
      const chunks: Buffer[] = [];
      sock.on("data", (b: Buffer) => {
        chunks.push(b);
        const all = Buffer.concat(chunks);
        const req = parseRequest(all);
        // request bytes consumed; the winsz frame follows immediately
        let off = 1 + 4;
        for (const a of req.argv) off += 4 + Buffer.byteLength(a);
        off += 4;
        for (const e of req.env) off += 4 + Buffer.byteLength(e);
        if (all.length >= off + 5 + 4 && !firstFrame) {
          firstFrame = all.subarray(off, off + 5 + 4);
          sock.write(frame(CH.EXIT, Buffer.from([0, 0, 0, 0])));
        }
      });
    });

    await new Promise<number>(async (resolve) => {
      await AgentConnection.connect(
        "127.0.0.1",
        port,
        { argv: ["/bin/sh"], tty: true, size: { cols: 100, rows: 30 } },
        { onExit: resolve },
      );
    });

    expect(firstFrame).toBeDefined();
    expect(firstFrame![0]).toBe(CH.WINSZ);
    expect(firstFrame!.readUInt16LE(5)).toBe(30); // rows
    expect(firstFrame!.readUInt16LE(7)).toBe(100); // cols
  });
});
