# bsdkrun (Clojure SDK)

[![Clojars Project](https://img.shields.io/clojars/v/io.github.tsirysndr/bsdkrun.svg)](https://clojars.org/io.github.tsirysndr/bsdkrun)

A Clojure SDK for [**bsdkrun**](https://github.com/tsirysndr/bsdkrun) — a
Firecracker-style microVM launcher for **BSD, Linux, and unikernel** guests on macOS and
Linux, built on [libkrun](https://github.com/containers/libkrun). Boot and
drive microVMs programmatically, inspired by the **Vercel** and **Deno**
Sandbox SDKs.

The SDK shells out to the `bsdkrun` binary, so it has effectively **zero
runtime dependencies** — just `org.clojure/clojure` itself and
`org.clojure/data.json` for parsing the CLI's JSON output. There is no
object hierarchy: a "sandbox" is a plain map (`{:id "..." :ssh-port 2222}`),
and every namespace is a set of functions over plain maps.

```clojure
(require '[bsdkrun.sandbox :as sandbox])

(def sbx (sandbox/create! {:os :linux :image "alpine"}))

;; exec argv directly, with env / stdin / a PTY / a working dir:
(println (:stdout (sandbox/exec! sbx ["uname" "-a"])))
(sandbox/exec! sbx ["apk" "add" "curl"] {:throw-on-error true})
(sandbox/run-command! sbx "curl" ["-fsSL" "https://example.com"])

(sandbox/stop! sbx)
```

## Install

Via `deps.edn`:

```clojure
io.github.tsirysndr/bsdkrun {:mvn/version "0.2.2"}
```

Via Leiningen:

```clojure
[io.github.tsirysndr/bsdkrun "0.2.2"]
```

### The `bsdkrun` binary

You need the `bsdkrun` binary itself. The SDK finds it via, in order:

1. `(bsdkrun.binary/set-override! "/path/to/bsdkrun")`
2. the `BSDKRUN_BIN` environment variable
3. `bsdkrun` on your `PATH`
4. an in-repo `target/release/bsdkrun` or `target/debug/bsdkrun` build (only
   relevant if you're working inside the `bsdkrun` monorepo itself)

See the [bsdkrun README](../../README.md) for installing the binary
(Homebrew on macOS, or build from source on Linux/KVM). This SDK assumes
libkrun is already linked — it does not auto-provision it.

## Creating a sandbox

`sandbox/create!` is discriminated on `:os` — the options map changes shape
per guest kind. Every `create!` runs the machine **detached** and returns a
handle: `{:id "..." :ssh-port ...}` (`:ssh-port` is set only when the boot
banner reported one, which only BSD guests do).

```clojure
(require '[bsdkrun.sandbox :as sandbox])

;; Linux OCI image (docker run-style)
(sandbox/create!
 {:os :linux
  :image "ghcr.io/owner/name:tag"
  :cpus 2
  :mem 1024
  :volume "web"                              ; persistent CoW rootfs
  :mounts ["~/project:/src" "~/data:/data:ro"]
  :attach-disk ["data.raw" "blobs.raw:ro"]   ; extra virtio-blk disks
  :net {:ports ["8080:80" "2222:22"]}
  :command ["node" "server.js"]})            ; args after `--`

;; FreeBSD (EFI on macOS, PVH on Linux/amd64)
(sandbox/create! {:os :freebsd :version "14.3" :mem 2048})

;; NetBSD (direct-kernel boot everywhere)
(sandbox/create! {:os :netbsd :version "10.1" :volume "db"})

;; Boot a raw disk through its UEFI loader
(sandbox/create! {:os :firmware :firmware "KRUN_EFI.fd" :disk "disk.raw"})

;; Boot a kernel directly, no bootloader
(sandbox/create! {:os :kernel :kernel "netbsd" :format "elf" :disk "root.raw"})
```

### Environment variables

`:env` sets the guest environment for the machine's entrypoint. It is merged
over the image's own config, so a key the image already defines is replaced
rather than duplicated.

```clojure
(def sbx
  (sandbox/create {:os "linux"
                   :image "node:22"
                   :env {"NODE_ENV" "production" "PORT" "3000"}
                   :command ["node" "server.js"]}))
```

Linux guests only — BSD guests boot their own init, so there is no generated
init to export into; set those from `exec` after boot. For a single command
rather than the whole machine, `exec` takes its own `:env`.

## Running commands

`sandbox/exec!` is the primary programmatic entrypoint. No shell parsing —
pass an argv vector (or a bare program name plus `:args`).

```clojure
(require '[bsdkrun.types :as types]
         '[clojure.java.io :as io])

(sandbox/exec! sbx ["ls" "-la" "/etc"])

(sandbox/exec! sbx "ruby"
  {:args ["-e" "puts ENV['X']"]
   :env {"X" "hi"}
   :cwd "/app"
   :stdin "data on stdin"
   :on-stdout #(io/copy (java.io.ByteArrayInputStream. %) System/out)
   :on-stderr #(io/copy (java.io.ByteArrayInputStream. %) System/err)
   :tty true                    ; allocate a PTY
   :throw-on-error true})       ; throw on non-zero exit (default: false)

;; Vercel-Sandbox-style alias:
(def result (sandbox/run-command! sbx "uname" ["-a"]))
(:stdout result)        ; raw stdout
(types/text result)     ; stdout, trailing newlines trimmed
(:exit-code result)
(types/ok? result)      ; true on exit 0
(types/lines result)    ; non-empty stdout lines
```

`exec!` returns a plain Result map (`{:stdout ... :stderr ... :exit-code ...
:command ...}`). It only throws when you pass `:throw-on-error true` (or
call `(bsdkrun.types/throw-if-failed! result)` yourself).

The callbacks receive byte arrays as chunks arrive, while `:stdout` and
`:stderr` remain fully buffered in the result. Streaming is independent of
`:tty`; a PTY changes command semantics and may merge stderr into stdout.

## Caching

`bsdkrun.cache` saves a guest directory under a key and restores it later, so a
rebuild can pick up where the last one left off. **A miss is not an error** —
check `:restored` rather than catching.

```clojure
(require '[bsdkrun.cache :as cache])

(let [k (str "deps-" lock-hash)
      hit (cache/restore "web" {:key k :restore-keys ["deps-"]})]
  (when-not (:restored hit)
    (sandbox/exec "web" ["npm" "ci"])
    (cache/save "web" "/app/node_modules" {:key k :compression "zstd"})))

(cache/ls)        ; every stored entry, newest first
(cache/rm [k])    ; or (cache/rm [] {:all true})
```

`:restore-keys` are prefixes tried in order when the exact key misses; within a
prefix the newest matching entry wins, and `:key` on the result says which one
was used. Formats are `"gzip"` (default), `"zstd"`, `"estargz"` and `"none"`.

Where entries live is host configuration, not an SDK concern: the default is
this host's disk, and `BSDKRUN_CACHE_BACKEND=s3` + `BSDKRUN_CACHE_S3_*` (or
`~/.config/bsdkrun/cache.toml`) points them at a bucket instead.

## Files

`bsdkrun.filesystem` reads and writes files in the guest. Parent directories are
created for you, and everything is byte-exact — `read-file` returns a byte array.

```clojure
(require '[bsdkrun.filesystem :as fs])

(fs/write-file "web" "/app/main.py" "print('hi')")
(fs/write-file "web" "/app/logo.png" png-bytes)

(fs/read-text "web" "/app/out.json")
(fs/read-file "web" "/app/logo.png")

(fs/upload "web" "./src" "/app/src")                     ; file or directory
(fs/download "web" "/app/dist" "./dist" {:recursive true})
```

`upload` looks at the local path to decide whether to recurse; `download` cannot
(the path is in the guest), so say so for a directory. A directory's *contents*
land in the destination: uploading `./src` to `/app/src` leaves the guest's
`/app/src` holding what `./src` holds.

Failures throw an `ex-info` whose `ex-data` is
`{:bsdkrun/error :file-transfer-failed :path ...}`.

> Transfers ride the same in-guest agent as `exec`, so the sandbox must be
> running. A directory copy also needs `tar` in the guest; single files need
> only the shell every bootable image already has.

## Lifecycle & inventory

```clojure
(require '[bsdkrun.sandbox :as sandbox])

(def sbx  (sandbox/create! {:os :linux :image "alpine" :name "web-1" :command ["sleep" "300"]}))
(def same (sandbox/get (:id sbx)))          ; reconnect (id prefix ok)
(def same (sandbox/get "web-1"))            ; ...or by its exact --name
(def all  (sandbox/list {:all true}))       ; vector of sandbox-info maps

(sandbox/status sbx)          ; sandbox-info map, or nil
(sandbox/running? sbx)        ; true / false
(sandbox/logs sbx)            ; console log (string)
(sandbox/shell! sbx)          ; interactive shell (inherits the terminal)
(sandbox/stop! sbx)           ; BSD guests clean-poweroff; Linux SIGTERM
(sandbox/start! sbx)          ; restart in place — resumes its own disk/rootfs
(sandbox/update! sbx {:cpus 4 :mem 2048})   ; applies on next start
(sandbox/remove! sbx {:force true})
```

Every function above takes a `ref` — a sandbox map (`sbx`) **or** a bare
id/name string — so you never have to reconnect first just to act on a
machine you already know the name of:

```clojure
(sandbox/exec! "web-1" ["uname" "-a"])      ; no (sandbox/get "web-1") needed first
(sandbox/stop! "web-1")
```

And since lifecycle functions with nothing interesting to return hand back
`ref` itself instead of `nil`, they thread with `->` — and with `doto`, when
you want the sandbox back at the end instead of the last call's result:

```clojure
;; -> : each step's result feeds the next; end on the exec output
(-> (sandbox/get "web-1")
    sandbox/start!
    (sandbox/exec! ["uname" "-a"])
    :stdout)

;; doto : same sbx driven through every step; you get the sbx back
(doto (sandbox/get "web-1")
  sandbox/start!
  (sandbox/exec! ["setup.sh"] {:throw-on-error true})
  sandbox/stop!)
```

Host-level namespaces:

```clojure
(require '[bsdkrun.system :as system]
         '[bsdkrun.images :as images]
         '[bsdkrun.volumes :as volumes]
         '[bsdkrun.networks :as networks])

(system/probe)                              ; toolchain sanity check -> boolean
(images/list)                               ; vector of image-info maps
(volumes/list)                              ; vector of volume-info maps
(volumes/remove! "web" {:force true})
(networks/list)                             ; vector of network-info maps
(system/fetch-image! :freebsd {:version "14.3"})
(system/versions :netbsd)                   ; vector of strings
```

## Networking, SSH & Tailscale

```clojure
;; forward ports at create time
(sandbox/create! {:os :linux :image "alpine" :net {:ports ["2222:22"]}})

;; agent-managed key-based SSH
(sandbox/ssh-setup! sbx)                              ; install local ~/.ssh/*.pub keys
(sandbox/ssh-setup! sbx {:user "tsiry" :key "~/.ssh/work.pub"})

;; put a guest on your tailnet
(sandbox/tailscale-up! sbx {:authkey "tskey-auth-..." :hostname "web"})
```

### Global networks — reach machines by name

Opt machines into a **shared network** so they get distinct IPs on one
subnet and reach each other **by IP and by name** (docker-compose style),
with internal DNS:

```clojure
(require '[bsdkrun.networks :as networks]
         '[bsdkrun.sandbox :as sandbox])

(networks/create! "devnet")

;; either set :net directly, or build it up with with-volume / with-network
;; while chaining into create! — handy alongside other with-* / opts wrangling:
(def db (sandbox/create!
         {:os :linux :image "alpine" :name "db"
          :net {:network "devnet"} :command ["sleep" "3600"]}))
(def api (-> {:os :linux :image "alpine" :name "api" :command ["sleep" "3600"]}
             (sandbox/with-volume "api-data")   ; persistent rootfs — create-time only
             (sandbox/with-network "devnet")    ; joins the shared subnet at boot
             sandbox/create!))

;; api reaches db by name over the shared subnet
(sandbox/exec! api ["ping" "-c1" "db"] {:throw-on-error true})

;; inspect + manage
(networks/list)                     ; vector of network-info maps
(networks/members "devnet")         ; vector of sandbox-info maps on the network
(def info (sandbox/status db))      ; (:network info) => "devnet", (:net-ip info) set

;; edit membership (applies on next start — a VM's NIC is fixed at boot)
(sandbox/connect-network! api "devnet")   ; or (networks/connect! (:id api) "devnet")
(sandbox/disconnect-network! api)
(sandbox/start! api)                      ; re-joins with the new membership

(networks/sync! "devnet")           ; refresh members' /etc/hosts (fixes NetBSD name lookup)
(networks/remove! "devnet" {:force true})
```

Names resolve on Linux and FreeBSD via the network's DNS; **NetBSD**
resolves via a synced `/etc/hosts` block — joins auto-sync, and
`networks/sync!` refreshes an existing network without restarting members.

`with-network` sets `:net {:network ...}` on a *create-options* map, for
chaining into a not-yet-created machine; `connect-network!`/
`disconnect-network!` edit an *existing* machine's membership instead (both
apply on the machine's next `start!`, since a VM's NIC is fixed at boot).
There is no `with-*` equivalent for `:attach-disk`/volumes beyond `create!`
— `bsdkrun` has no way to attach a volume to an already-booted machine, only
to choose one (`with-volume`, or a literal `:volume`) before it boots.

## Namespaces

- `bsdkrun.sandbox` — the machine lifecycle: `create!`, `get`, `list`, `id`,
  `with-volume`, `with-network`, `exec!`, `run-command!`, `logs`, `shell!`,
  `status`, `running?`, `stop!`, `start!`, `remove!`, `update!`,
  `connect-network!`, `disconnect-network!`, `ssh-setup!`, `tailscale-up!`.
- `bsdkrun.images` / `bsdkrun.volumes` / `bsdkrun.networks` / `bsdkrun.system`
  — host-level inventory and toolchain operations.
- `bsdkrun.args` — the pure argv-builder behind `sandbox/create!`
  (`build-create-args` and friends), handy if you want to see or reuse the
  exact CLI invocation without running it.
- `bsdkrun.types` — pure JSON-row -> map decoders (`sandbox-info-from-row`
  etc.) and the Result-map helpers (`ok?`, `text`, `json`, `lines`,
  `throw-if-failed!`).
- `bsdkrun.binary` — binary discovery (`resolve`, `set-override!`, `reset!`).
- `bsdkrun.process` — the low-level subprocess layer (`run`, `run!`,
  `spawn-interactive!`), if you need to shell out to a `bsdkrun` subcommand
  this SDK doesn't wrap yet.
- `bsdkrun.client` — the remote GraphQL client sibling to `bsdkrun.sandbox`,
  talking to a [`bsdkrund`](../../daemon/README.md) daemon instead of
  shelling out locally. See
  [Connecting to a remote daemon](#connecting-to-a-remote-daemon) below.
- `bsdkrun.errors` — the `ex-info` constructors every namespace above throws.

There is no `bsdkrun.core` "front door" namespace — `require` the namespace
you need with a short alias (as in every example above); that's idiomatic
Clojure and keeps call sites unambiguous about which host-level resource
they're touching.

## Connecting to a remote daemon

Everything above talks to a local `bsdkrun` binary. `bsdkrun.client` is the
network sibling: it drives the same operations against a remote
[`bsdkrund`](../../daemon/README.md) over its GraphQL API — no local binary
needed, just a URL and a bearer token. Every function takes a `client` map
first, exactly like `bsdkrun.sandbox`'s functions take a `sandbox` map first.

```clojure
(require '[bsdkrun.client :as client])

(def c (client/new-client {:url "http://vps.example.com:50052" :token "9f2c..."}))
;; or, from BSDKRUN_URL / BSDKRUN_TOKEN:
(def c (client/client-from-env))

(def machines (client/list-machines c {:all true}))  ; same SandboxInfo shape sandbox/list returns
(def id (client/run-linux! c {:image "alpine" :cpus 2 :mem 1024 :command ["sleep" "300"]}))

(def result (client/exec! c id ["uname" "-a"]))
(println (String. (:output result) "UTF-8") (:exit-code result))

(client/stop! c id)
(client/remove! c [id])
```

`run-linux!`/`run-bsd!`/`run-nanos!`/`run-unikraft!`/`run-solo5!`/`run-osv!`/`run-flavor!`
each take the same options as the corresponding GraphQL mutation
(`daemon/src/graphql.rs`) — kebab-case keys mapped 1:1 onto the wire's
camelCase fields (`:kernel-version` -> `kernelVersion`, `:attach-disk` ->
`attachDisk`, etc.) — and return the new machine's id.
`stop!`/`start!`/`remove!`/`update!`/`commit!` return a command-result map
(`{:exit-code :stdout :stderr}`).

### Snapshots

A snapshot is a **copy-on-write clone of a machine's disk state** — instant to
take, free until the two sides diverge. `branch` boots a new machine from one
(or from a machine, which is snapshotted first); `restore`/`rollback` put one
back, leaving the machine stopped. A BSD guest is powered off to snapshot it:
a mounted UFS cannot be cloned consistently.

```clojure
(def snap (client/snapshot! c id {:name "before-upgrade"}))
(client/snapshots c {:machine id})            ; newest first
(client/branch! c (:name snap) {:name "web-test"})
(client/restore! c id (:name snap))           ; or (client/rollback! c id)
(client/remove-snapshots! c (:name snap))
```

### Docker

bsdkrun runs one `docker:dind` microVM and serves its API on a host unix
socket, so the host's own `docker` CLI drives the same engine these calls do.
Starting is idempotent — the VM has a fixed name, so it resumes rather than
creating a second.

```clojure
(def status (client/docker-start! c {:cpus 4 :mem 4096}))
(println (:socket status))
(doseq [x (client/docker-containers c)] (println (:name x) (:state x) (:ports x)))
(client/docker-container! c "restart" "web")
(println (client/docker-logs c "web" {:tail 50}))
```

For a live terminal instead of a one-shot `exec!`, use `shell!`:

```clojure
(def session (client/shell! c id))  ; or {:command [...]} for a non-login command
((:on-output! session) (fn [bytes] (.write System/out bytes)))
((:on-exit! session) (fn [code] (println "\nexited" code)))
((:write! session) "ls -la\n")
((:resize! session) 50 120)
((:close! session))
```

`(client/follow-logs c id {:on-data (fn [bytes] ...)})` streams a machine's
console live instead of the one-shot `(client/logs c id)`. Both
`exec!`/`shell!` and `follow-logs` are built on the same
`openShell`/`shellOutput` shell-session protocol the daemon uses for every
interactive terminal — see
[`daemon/README.md`](../../daemon/README.md#interactive-shells-over-graphql)
for the wire-level story.

Not every GraphQL operation has a typed method yet (flavor/network/volume
management, for instance) — `(client/request c query variables)` runs any
raw query or mutation, and `(client/subscribe c query variables handlers)`
runs any raw subscription, for anything not wrapped above.

Unlike every other bsdkrun SDK, `bsdkrun.client` needs **zero extra
dependencies** for its transport: `java.net.http.HttpClient` handles the
queries/mutations, and its built-in `java.net.http.WebSocket` speaks
`graphql-transport-ws` for subscriptions (`exec!`/`shell!`/`follow-logs`) —
both are core JDK APIs (Java 11+). TypeScript/Python/Ruby/Elixir/Gleam each
had to hand-roll RFC 6455 WebSocket framing because their standard library
had no client of its own; the JVM does, so this namespace doesn't.

`new-client`/`client-from-env` both reject a URL configured without a token
rather than silently making an unauthenticated request — set both
`BSDKRUN_URL` and `BSDKRUN_TOKEN`, or pass both explicitly.

## Errors

Every error is a plain `ex-info` — there is no exception class hierarchy.
Pattern-match on `(:bsdkrun/error (ex-data e))`:

```clojure
(require '[bsdkrun.sandbox :as sandbox])

(try
  (sandbox/exec! sbx ["false"] {:throw-on-error true})
  (catch clojure.lang.ExceptionInfo e
    (case (:bsdkrun/error (ex-data e))
      :command-failed (println "exit" (:exit-code (ex-data e)) (:stderr (ex-data e)))
      (throw e))))
```

Error kinds:

- `:binary-not-found` — the `bsdkrun` binary wasn't found (`ex-data` carries
  `:searched`, the ordered list of candidates that were probed).
- `:command-failed` — a command exited non-zero (`ex-data` carries
  `:exit-code`, `:stdout`, `:stderr`, `:command`). Thrown by `exec!` with
  `:throw-on-error true`, by the lifecycle/namespace helpers, and by the
  agent helpers (`ssh-setup!`, `tailscale-up!`).
- `:sandbox-not-found` — `sandbox/get` matched no machine (`ex-data` carries
  `:id`).
- `:missing-option` — a required `create!` option (`:image`, `:kernel`,
  `:firmware`, `:disk`, depending on `:os`) was left out.
- `:unknown-os` — `sandbox/create!` / `args/build-create-args` was called
  with an `:os` the SDK doesn't know how to build argv for.
- `:graphql-error` — a `bsdkrun.client` request to a remote daemon failed —
  a transport failure, a non-JSON response, or any GraphQL error that isn't
  an auth failure (`ex-data` carries `:code`, the daemon's `extensions.code`,
  when there is one).
- `:auth-error` — the daemon rejected the bearer token: an HTTP 401, a
  GraphQL error whose `extensions.code` is `"UNAUTHENTICATED"`, or a
  websocket that closed before `connection_ack` ever arrived.
- `:missing-config` — `client/client-from-env` was called with
  `BSDKRUN_URL` unset, or set without `BSDKRUN_TOKEN`.

## Development

Needs a JDK (21+) and the [Clojure CLI](https://clojure.org/guides/install_clojure)
— pinned via [`mise`](https://mise.jdx.dev) (`mise install`).

```sh
mise install           # JDK 21 + the Clojure CLI (or install them yourself)
clj -M:test             # run the test suite
clj -M:rebel            # a syntax-highlighting, autocompleting REPL with the SDK preloaded
clj -T:build jar        # build target/bsdkrun-<version>.jar
clj -T:build install    # install to the local ~/.m2
clj -T:build deploy     # deploy to Clojars (needs CLOJARS_USERNAME / CLOJARS_PASSWORD)
```

Tests are unit tests only — argv building, JSON-row decoding, binary
discovery (via dependency injection, not real subprocesses or environment
mutation), and `bsdkrun.client`'s HTTP/websocket transport (driven against a
from-scratch local HTTP+WS server on loopback, not a real `bsdkrund`).
Nothing spawns a real `bsdkrun` binary, talks to a real daemon, or needs a
hypervisor.

`clj -M:rebel` starts [rebel-readline](https://github.com/bhauman/rebel-readline)
(syntax highlighting, structural editing, inline docs/autocomplete) in the
`user` namespace, with `dev/user.clj` preloading `bsdkrun.sandbox`,
`.images`/`.volumes`/`.networks`/`.system`/`.args`/`.types`/`.errors`, and
`bsdkrun.client` under short aliases — mirrors `sdk/ruby/bin/console`'s
preload. `(ps)` lists every machine (exited ones included); `(last-machine)`
is the newest one. Needs a real terminal (rebel-readline reads raw
keystrokes for its interactive editing, so it refuses to start under piped
input or a non-tty process). `dev/` is dev-only — it is not part of `:paths`
and is not packaged into the published jar.

## License

MIT
