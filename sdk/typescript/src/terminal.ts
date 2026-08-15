import {
  AgentConnection,
  type AgentConnectOptions,
} from "./agent-protocol.js";

/** Options for opening an interactive {@link Terminal}. */
export interface TerminalOptions {
  /** Command to run. Defaults to an interactive `/bin/sh`. */
  command?: string[];
  /** Environment variables for the session. */
  env?: Record<string, string>;
  /** Initial terminal size. Defaults to 80×24. */
  cols?: number;
  rows?: number;
  /** Host the agent is forwarded on (default `127.0.0.1`). */
  host?: string;
}

/**
 * A minimal duplex sink an xterm.js `Terminal` satisfies (`onData`, `write`),
 * so {@link Terminal.pipe} can wire the two together in one call — both in the
 * browser (real xterm.js) and server-side (node-pty, a socket, a test double).
 */
export interface DataSink {
  /** Guest → sink: called with each chunk of guest output (as a string). */
  write(data: string): void;
  /** Sink → guest: register a handler for user input. */
  onData(handler: (data: string) => void): unknown;
  /** Optional: react to guest-driven resizes. */
  resize?(cols: number, rows: number): void;
}

/** A duck-typed WebSocket (browser `WebSocket` and the `ws` package both fit). */
export interface WebSocketLike {
  send(data: string | ArrayBufferView): void;
  close(): void;
  addEventListener?(type: "message", cb: (ev: { data: unknown }) => void): void;
  on?(event: "message", cb: (data: unknown) => void): void;
}

/**
 * An interactive PTY session in the guest, streamed over the agent's TCP
 * protocol. Shaped for a terminal front-end: byte streams in/out plus dynamic
 * {@link resize}. Feed `onData` output into xterm.js and forward its keystrokes
 * to {@link write}; call {@link resize} from xterm's `onResize`.
 *
 * ```ts
 * const term = await sbx.terminal({ cols: 120, rows: 30 });
 * term.onData((chunk) => xterm.write(chunk));
 * xterm.onData((input) => term.write(input));
 * xterm.onResize(({ cols, rows }) => term.resize(cols, rows));
 * ```
 */
export class Terminal {
  #conn: AgentConnection | null = null;
  #dataCbs = new Set<(data: Buffer) => void>();
  #exitCbs = new Set<(code: number) => void>();
  #exitCode: number | null = null;
  #exitResolve!: (code: number) => void;
  /** Resolves with the exit code when the session ends. */
  readonly exited: Promise<number>;

  private constructor() {
    this.exited = new Promise((res) => {
      this.#exitResolve = res;
    });
  }

  /** @internal — open a terminal against an already-resolved agent port. */
  static async open(
    host: string,
    port: number,
    id: string,
    opts: TerminalOptions,
  ): Promise<Terminal> {
    const term = new Terminal();
    const cols = opts.cols ?? 80;
    const rows = opts.rows ?? 24;
    const env = Object.entries(opts.env ?? {}).map(([k, v]) => `${k}=${v}`);
    const connectOpts: AgentConnectOptions = {
      argv: opts.command ?? ["/bin/sh"],
      env,
      tty: true,
      size: { cols, rows },
    };
    term.#conn = await AgentConnection.connect(
      opts.host ?? host,
      port,
      connectOpts,
      {
        onStdout: (d) => term.#emit(d),
        onStderr: (d) => term.#emit(d), // a PTY merges stderr into stdout
        onExit: (code) => term.#onExit(code),
        onError: () => term.#onExit(term.#exitCode ?? -1),
      },
    );
    // `id` retained for clearer errors by callers; not otherwise needed here.
    void id;
    return term;
  }

  #emit(data: Buffer): void {
    for (const cb of this.#dataCbs) cb(data);
  }

  #onExit(code: number): void {
    if (this.#exitCode !== null) return;
    this.#exitCode = code;
    for (const cb of this.#exitCbs) cb(code);
    this.#exitResolve(code);
  }

  /**
   * Subscribe to guest output. The chunk is a `Buffer`; call `.toString()` for
   * text (xterm.js `write` accepts either a string or Uint8Array).
   */
  onData(handler: (data: Buffer) => void): () => void {
    this.#dataCbs.add(handler);
    return () => this.#dataCbs.delete(handler);
  }

  /** Subscribe to session exit. */
  onExit(handler: (code: number) => void): () => void {
    if (this.#exitCode !== null) handler(this.#exitCode);
    else this.#exitCbs.add(handler);
    return () => this.#exitCbs.delete(handler);
  }

  /** Write user input (keystrokes) to the guest. */
  write(data: string | Uint8Array): void {
    this.#conn?.write(data);
  }

  /** Update the guest PTY size. Call from xterm's `onResize`. */
  resize(cols: number, rows: number): void {
    this.#conn?.resize(cols, rows);
  }

  /** End the session. */
  kill(): void {
    this.#conn?.close();
    this.#onExit(this.#exitCode ?? -1);
  }

  /** The exit code once finished, else null. */
  get exitCode(): number | null {
    return this.#exitCode;
  }

  /**
   * Wire this terminal to an xterm.js-like sink in one call: guest output is
   * written to the sink, and the sink's input is forwarded to the guest.
   * Returns a disposer that unsubscribes (it does not kill the session).
   */
  pipe(sink: DataSink): () => void {
    const off = this.onData((chunk) => sink.write(chunk.toString("utf8")));
    sink.onData((input) => this.write(input));
    return off;
  }

  /**
   * Bridge this terminal to a WebSocket (browser `WebSocket` or the `ws`
   * package): binary/echo guest output is sent as messages, and inbound
   * messages are written to the guest. A JSON message `{"resize":[cols,rows]}`
   * triggers a resize. Returns a disposer.
   */
  bindWebSocket(ws: WebSocketLike): () => void {
    const off = this.onData((chunk) => {
      try {
        ws.send(chunk.toString("utf8"));
      } catch {
        /* socket closing */
      }
    });
    const handle = (raw: unknown) => {
      const text =
        typeof raw === "string" ? raw : Buffer.from(raw as ArrayBuffer).toString("utf8");
      if (text.startsWith("{") && text.includes("resize")) {
        try {
          const msg = JSON.parse(text) as { resize?: [number, number] };
          if (msg.resize) {
            this.resize(msg.resize[0], msg.resize[1]);
            return;
          }
        } catch {
          /* not a control message — fall through as data */
        }
      }
      this.write(text);
    };
    if (ws.addEventListener) ws.addEventListener("message", (ev) => handle(ev.data));
    else ws.on?.("message", handle);
    this.onExit(() => ws.close());
    return off;
  }
}
