# Browser terminal (xterm.js)

A real interactive shell inside a bsdkrun microVM, rendered in the browser with
[xterm.js](https://xtermjs.org). The SDK's `Terminal` speaks the guest agent's
TCP protocol directly, so **keystrokes, output, and live window-resize** all
work — no host TTY, no CLI PTY.

```
bun run examples/08-browser-terminal/server.ts
# open http://localhost:3000
```

## How it fits together

```
xterm.js (browser)  ──WebSocket──▶  Bun server  ──TCP (agent proto)──▶  guest PTY
        ▲ onData / onResize            Terminal          via gvproxy-forwarded port
        └───────────── output ─────────────┘
```

- The server boots an Alpine sandbox and, per WebSocket connection, opens
  `box.terminal({ command: ["/bin/sh"] })`.
- `term.onData(chunk => ws.send(chunk))` streams guest output to the browser;
  the page writes it into xterm.
- Browser input is sent over the socket and `term.write(text)` forwards it to
  the guest.
- xterm's `onResize` sends `{"resize":[cols,rows]}`, which the server maps to
  `term.resize(cols, rows)` — a live PTY resize frame.

The same `Terminal` works with the `ws` npm package on Node/Deno via
`term.bindWebSocket(ws)`, which handles the output, input, and resize wiring for
you.
