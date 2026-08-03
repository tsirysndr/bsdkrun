# Examples

Runnable examples for `@bsdkrun/sdk`. They import from `../src` so they work
straight from a checkout. Each needs the `bsdkrun` binary discoverable (on
`PATH`, via `BSDKRUN_BIN`, or a local `target/{release,debug}/bsdkrun` build).

Run with any supported runtime:

```sh
bun  run examples/01-hello-linux.ts
deno run -A examples/01-hello-linux.ts
# node needs a TS loader, or build first: `bun run build` then import from ../dist
```

| File | Shows |
| --- | --- |
| [`01-hello-linux.ts`](./01-hello-linux.ts) | Boot Alpine, `sh` + `exec`, stop |
| [`02-exec-advanced.ts`](./02-exec-advanced.ts) | `exec` with env, stdin, cwd, exit codes |
| [`03-sh-template.ts`](./03-sh-template.ts) | `sh` quoting, chaining, `raw()`, `.nothrow()` |
| [`04-lifecycle-and-logs.ts`](./04-lifecycle-and-logs.ts) | create / list / get / status / logs / stop |
| [`05-ssh-and-ports.ts`](./05-ssh-and-ports.ts) | Port forwarding + agent-managed SSH |
| [`06-bsd-guests.ts`](./06-bsd-guests.ts) | FreeBSD / NetBSD guests |
| [`07-images-and-volumes.ts`](./07-images-and-volumes.ts) | `images`, `volumes`, `probe` |
