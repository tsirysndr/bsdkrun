# bsdkrun web UI

The desktop app's interface, in a browser, talking to a [`bsdkrund`](../daemon)
GraphQL API instead of a local binary.

Because it drives a daemon over the network, one page can manage bsdkrun on a
remote VPS or bare-metal KVM host — the thing the desktop app cannot do.

## Running it

The UI is compiled into the `bsdkrun` binary, so there is nothing to install:

```console
$ bsdkrun ui                    # serves on http://127.0.0.1:8088 and opens a browser
$ bsdkrund --graphql-bind 127.0.0.1:50052   # on the machine with the VMs
```

On first run the UI asks for the GraphQL URL and the access token the daemon
printed. Both are changeable later from Settings, and stored in this browser's
local storage.

The daemon does not have to be on the same host as the UI — CORS is open, since
the API is guarded by a bearer token rather than by origin.

## Same UI as the desktop app

This is a port, not a rewrite. 21 of the 26 components are byte-identical to
`desktop/src`, and `index.css` plus the Tailwind theme are unchanged, so the two
look the same by construction rather than by careful re-implementation.

Only the transport differs. Five files needed changes:

| File               | Why                                                        |
| ------------------ | ---------------------------------------------------------- |
| `lib/api.ts`       | GraphQL instead of Tauri `invoke`, same exported surface    |
| `SettingsModal`    | Configures a daemon URL + token, not a binary + cache path  |
| `CliModal`, `useShortcuts` | `window.open` instead of the Tauri opener plugin    |
| `LogsPane`, `TerminalPane` | `UnlistenFn` imported from `lib/api`                |
| `VolumesView`      | A volume's size is text from the CLI, not a byte count      |

Keeping `api.ts`'s shape identical is what makes that possible: every view,
dialog and hook is shared verbatim.

## Streaming

Three things stream, all over one `graphql-ws` socket:

- **Terminals** — `openShell` then a `shellOutput` subscription, with keystrokes
  and resizes sent back as mutations. A GraphQL subscription only flows
  server→client, so input cannot travel on it.
- **Logs** — a `machineLogs` subscription, re-published as the `log://line`
  events the components already expected.
- **Launch progress** — `launchLinux` / `launchBsd` / `launchFlavor` /
  `buildFlavor` stream provisioning output and end with the new machine id.

`lib/api.ts` republishes all of it through a small event bus so the components,
written against Tauri's `listen()`, work unchanged.

## Development

```console
$ bun install
$ bun run dev        # http://localhost:5173, pointed at any daemon
$ bun run typecheck
$ bun run build      # -> web/dist, embedded by the Rust build
```

`web/dist` is gitignored. `build.rs` writes a placeholder page there when it is
missing, so `cargo build` works in a checkout that has never run node — the
placeholder just explains how to build the real thing.
