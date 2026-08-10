# bsdkrun (Clojure SDK)

A Clojure SDK for [**bsdkrun**](https://github.com/tsirysndr/bsdkrun) — a
Firecracker-style microVM launcher for **BSD and Linux** guests on macOS and
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

(def box (sandbox/create! {:os "linux" :image "alpine"}))

;; exec argv directly, with env / stdin / a PTY / a working dir:
(println (:stdout (sandbox/exec! box ["uname" "-a"])))
(sandbox/exec! box ["apk" "add" "curl"] {:throw-on-error true})
(sandbox/run-command! box "curl" ["-fsSL" "https://example.com"])

(sandbox/stop! box)
```

## Install

Via `deps.edn`:

```clojure
io.github.tsirysndr/bsdkrun {:mvn/version "0.1.0"}
```

Via Leiningen:

```clojure
[io.github.tsirysndr/bsdkrun "0.1.0"]
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
 {:os "linux"
  :image "ghcr.io/owner/name:tag"
  :cpus 2
  :mem 1024
  :volume "web"                              ; persistent CoW rootfs
  :mounts ["~/project:/src" "~/data:/data:ro"]
  :net {:ports ["8080:80" "2222:22"]}
  :command ["node" "server.js"]})            ; args after `--`

;; FreeBSD (EFI on macOS, PVH on Linux/amd64)
(sandbox/create! {:os "freebsd" :version "14.3" :mem 2048})

;; NetBSD (direct-kernel boot everywhere)
(sandbox/create! {:os "netbsd" :version "10.1" :volume "db"})

;; Boot a raw disk through its UEFI loader
(sandbox/create! {:os "firmware" :firmware "KRUN_EFI.fd" :disk "disk.raw"})

;; Boot a kernel directly, no bootloader
(sandbox/create! {:os "kernel" :kernel "netbsd" :format "elf" :disk "root.raw"})
```

## Running commands

`sandbox/exec!` is the primary programmatic entrypoint. No shell parsing —
pass an argv vector (or a bare program name plus `:args`).

```clojure
(require '[bsdkrun.types :as types])

(sandbox/exec! box ["ls" "-la" "/etc"])

(sandbox/exec! box "ruby"
  {:args ["-e" "puts ENV['X']"]
   :env {"X" "hi"}
   :cwd "/app"
   :stdin "data on stdin"
   :tty true                    ; allocate a PTY
   :throw-on-error true})       ; throw on non-zero exit (default: false)

;; Vercel-Sandbox-style alias:
(def result (sandbox/run-command! box "uname" ["-a"]))
(:stdout result)        ; raw stdout
(types/text result)     ; stdout, trailing newlines trimmed
(:exit-code result)
(types/ok? result)      ; true on exit 0
(types/lines result)    ; non-empty stdout lines
```

`exec!` returns a plain Result map (`{:stdout ... :stderr ... :exit-code ...
:command ...}`). It only throws when you pass `:throw-on-error true` (or
call `(bsdkrun.types/throw-if-failed! result)` yourself).

## Lifecycle & inventory

```clojure
(require '[bsdkrun.sandbox :as sandbox])

(def box  (sandbox/create! {:os "linux" :image "alpine" :command ["sleep" "300"]}))
(def same (sandbox/get (:id box)))          ; reconnect (prefix ok)
(def all  (sandbox/list {:all true}))       ; vector of sandbox-info maps

(sandbox/status box)          ; sandbox-info map, or nil
(sandbox/running? box)        ; true / false
(sandbox/logs box)            ; console log (string)
(sandbox/shell! box)          ; interactive shell (inherits the terminal)
(sandbox/stop! box)           ; BSD guests clean-poweroff; Linux SIGTERM
(sandbox/start! box)          ; restart in place — resumes its own disk/rootfs
(sandbox/update! box {:cpus 4 :mem 2048})   ; applies on next start
(sandbox/remove! box {:force true})
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
(system/fetch-image! "freebsd" {:version "14.3"})
(system/versions "netbsd")                  ; vector of strings
```

## Networking, SSH & Tailscale

```clojure
;; forward ports at create time
(sandbox/create! {:os "linux" :image "alpine" :net {:ports ["2222:22"]}})

;; agent-managed key-based SSH
(sandbox/ssh-setup! box)                              ; install local ~/.ssh/*.pub keys
(sandbox/ssh-setup! box {:user "tsiry" :key "~/.ssh/work.pub"})

;; put a guest on your tailnet
(sandbox/tailscale-up! box {:authkey "tskey-auth-..." :hostname "web"})
```

### Global networks — reach machines by name

Opt machines into a **shared network** so they get distinct IPs on one
subnet and reach each other **by IP and by name** (docker-compose style),
with internal DNS:

```clojure
(require '[bsdkrun.networks :as networks]
         '[bsdkrun.sandbox :as sandbox])

(networks/create! "devnet")

(def db (sandbox/create!
         {:os "linux" :image "alpine" :name "db"
          :net {:network "devnet"} :command ["sleep" "3600"]}))
(def api (sandbox/create!
          {:os "linux" :image "alpine" :name "api"
           :net {:network "devnet"} :command ["sleep" "3600"]}))

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

## Namespaces

- `bsdkrun.sandbox` — the machine lifecycle: `create!`, `get`, `list`,
  `exec!`, `run-command!`, `logs`, `shell!`, `status`, `running?`, `stop!`,
  `start!`, `remove!`, `update!`, `connect-network!`, `disconnect-network!`,
  `ssh-setup!`, `tailscale-up!`.
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
- `bsdkrun.errors` — the `ex-info` constructors every namespace above throws.

There is no `bsdkrun.core` "front door" namespace — `require` the namespace
you need with a short alias (as in every example above); that's idiomatic
Clojure and keeps call sites unambiguous about which host-level resource
they're touching.

A `bsdkrun.client` namespace — a remote GraphQL client sibling to
`bsdkrun.sandbox`, talking to a [`bsdkrund`](../../daemon/README.md) daemon
instead of shelling out locally (mirroring the Ruby SDK's `Bsdkrun::Client`)
— is planned for a future release; nothing in this SDK's shape assumes it
can't be added alongside `bsdkrun.sandbox` later.

## Errors

Every error is a plain `ex-info` — there is no exception class hierarchy.
Pattern-match on `(:bsdkrun/error (ex-data e))`:

```clojure
(require '[bsdkrun.sandbox :as sandbox])

(try
  (sandbox/exec! box ["false"] {:throw-on-error true})
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

## Development

Needs a JDK (21+) and the [Clojure CLI](https://clojure.org/guides/install_clojure)
— pinned via [`mise`](https://mise.jdx.dev) (`mise install`).

```sh
mise install          # JDK 21 + the Clojure CLI (or install them yourself)
clj -M:test            # run the test suite
clj -T:build jar       # build target/bsdkrun-<version>.jar
clj -T:build install   # install to the local ~/.m2
clj -T:build deploy    # deploy to Clojars (needs CLOJARS_USERNAME / CLOJARS_PASSWORD)
```

Tests are unit tests only — argv building, JSON-row decoding, and binary
discovery (via dependency injection, not real subprocesses or environment
mutation). Nothing spawns a real `bsdkrun` binary or needs a hypervisor.

## License

MIT
