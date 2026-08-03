import { connect as netConnect, type Socket } from "node:net";
import { readFileSync } from "node:fs";
import { join } from "node:path";
import { BsdkrunError } from "./errors.js";

/**
 * A pure-TypeScript client for the in-guest exec agent's framed TCP protocol —
 * the same wire format `bsdkrun exec` speaks (see `src/agent.rs`). Talking to it
 * directly (instead of shelling out) gives full control of the stdin/stdout
 * streams and, crucially, **dynamic window-resize** frames — what an
 * xterm.js-backed browser terminal needs.
 *
 * Wire format, after connecting to the gvproxy-forwarded host port:
 *
 *   request  := tty:u8  argc:u32le (len:u32le bytes)*  envc:u32le (len:u32le bytes)*
 *   frame    := channel:u8  len:u32le  payload[len]
 *
 * All integers are little-endian. Channels: 0=stdin 1=stdout 2=stderr 3=exit
 * 4=winsz. A winsz payload is rows:u16le cols:u16le; exit is code:u32le; a
 * zero-length stdin frame signals EOF.
 */

export const CH_STDIN = 0;
export const CH_STDOUT = 1;
export const CH_STDERR = 2;
export const CH_EXIT = 3;
export const CH_WINSZ = 4;

/** Raised when the agent port file for a machine can't be found/read. */
export class AgentUnavailableError extends BsdkrunError {
  constructor(id: string, detail: string) {
    super(
      `the exec agent for sandbox ${id} is not reachable (${detail}). The ` +
        `machine needs networking up and its agent running.`,
    );
    this.name = "AgentUnavailableError";
  }
}

/** Read the gvproxy-forwarded host port for a machine from its state dir. */
export function readAgentPort(stateDir: string, id: string): number {
  try {
    const raw = readFileSync(join(stateDir, "agent.port"), "utf8").trim();
    const port = Number.parseInt(raw, 10);
    if (!Number.isInteger(port) || port <= 0) {
      throw new Error(`bad port ${JSON.stringify(raw)}`);
    }
    return port;
  } catch (err) {
    throw new AgentUnavailableError(
      id,
      `no agent.port in ${stateDir}: ${(err as Error).message}`,
    );
  }
}

function u32le(n: number): Buffer {
  const b = Buffer.allocUnsafe(4);
  b.writeUInt32LE(n >>> 0, 0);
  return b;
}

/** Encode the initial exec request (argv + env + tty flag). */
function encodeRequest(tty: boolean, argv: string[], env: string[]): Buffer {
  const parts: Buffer[] = [Buffer.from([tty ? 1 : 0]), u32le(argv.length)];
  for (const a of argv) {
    const s = Buffer.from(a, "utf8");
    parts.push(u32le(s.length), s);
  }
  parts.push(u32le(env.length));
  for (const e of env) {
    const s = Buffer.from(e, "utf8");
    parts.push(u32le(s.length), s);
  }
  return Buffer.concat(parts);
}

/** Encode one protocol frame. */
function encodeFrame(channel: number, payload: Buffer): Buffer {
  return Buffer.concat([Buffer.from([channel]), u32le(payload.length), payload]);
}

export interface AgentConnectOptions {
  /** argv to run in the guest. */
  argv: string[];
  /** Environment entries as `K=V` strings. */
  env?: string[];
  /** Request a PTY (interactive). */
  tty?: boolean;
  /** Initial terminal size (when `tty`). */
  size?: { cols: number; rows: number };
}

export interface AgentHandlers {
  onStdout?: (data: Buffer) => void;
  onStderr?: (data: Buffer) => void;
  onExit?: (code: number) => void;
  onError?: (err: Error) => void;
}

/**
 * A live connection to the guest agent running one command. Incremental frame
 * parser over a `node:net` socket; works on Node, Deno and Bun.
 */
export class AgentConnection {
  #socket: Socket;
  #buf: Buffer = Buffer.alloc(0);
  #handlers: AgentHandlers;
  #closed = false;

  private constructor(socket: Socket, handlers: AgentHandlers) {
    this.#socket = socket;
    this.#handlers = handlers;
    socket.on("data", (chunk: Buffer) => this.#onData(chunk));
    socket.on("error", (err) => this.#handlers.onError?.(err));
    socket.on("close", () => {
      if (!this.#closed) {
        this.#closed = true;
        // A close without an explicit exit frame — report a generic code.
        this.#handlers.onExit?.(-1);
      }
    });
  }

  /** Open a connection and send the exec request. */
  static connect(
    host: string,
    port: number,
    opts: AgentConnectOptions,
    handlers: AgentHandlers,
  ): Promise<AgentConnection> {
    return new Promise((resolve, reject) => {
      const socket = netConnect({ host, port }, () => {
        socket.setNoDelay(true);
        socket.write(encodeRequest(!!opts.tty, opts.argv, opts.env ?? []));
        const conn = new AgentConnection(socket, handlers);
        if (opts.tty && opts.size) {
          conn.resize(opts.size.cols, opts.size.rows);
        }
        resolve(conn);
      });
      socket.once("error", reject);
    });
  }

  #onData(chunk: Buffer): void {
    this.#buf = this.#buf.length ? Buffer.concat([this.#buf, chunk]) : chunk;
    // Parse as many complete frames as are buffered.
    while (this.#buf.length >= 5) {
      const channel = this.#buf[0]!;
      const len = this.#buf.readUInt32LE(1);
      if (this.#buf.length < 5 + len) break;
      const payload = this.#buf.subarray(5, 5 + len);
      this.#buf = this.#buf.subarray(5 + len);
      this.#dispatch(channel, payload);
    }
  }

  #dispatch(channel: number, payload: Buffer): void {
    switch (channel) {
      case CH_STDOUT:
        this.#handlers.onStdout?.(payload);
        break;
      case CH_STDERR:
        this.#handlers.onStderr?.(payload);
        break;
      case CH_EXIT: {
        const code = payload.length >= 4 ? payload.readUInt32LE(0) : 0;
        this.#closed = true;
        this.#handlers.onExit?.(code);
        this.#socket.end();
        break;
      }
      default:
        break; // ignore unknown channels
    }
  }

  /** Write bytes to the guest process's stdin. */
  write(data: Buffer | Uint8Array | string): void {
    if (this.#closed) return;
    const buf = Buffer.isBuffer(data) ? data : Buffer.from(data as Uint8Array);
    this.#socket.write(encodeFrame(CH_STDIN, buf));
  }

  /** Signal stdin EOF (a zero-length stdin frame). */
  endStdin(): void {
    if (this.#closed) return;
    this.#socket.write(encodeFrame(CH_STDIN, Buffer.alloc(0)));
  }

  /** Send a window-resize frame — call whenever the terminal is resized. */
  resize(cols: number, rows: number): void {
    if (this.#closed) return;
    const payload = Buffer.allocUnsafe(4);
    payload.writeUInt16LE(rows & 0xffff, 0);
    payload.writeUInt16LE(cols & 0xffff, 2);
    this.#socket.write(encodeFrame(CH_WINSZ, payload));
  }

  /** Close the connection. */
  close(): void {
    this.#closed = true;
    this.#socket.destroy();
  }
}
