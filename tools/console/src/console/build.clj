(ns console.build
  "Wrappers around the root `Makefile` — the CLI/daemon/supervisor build,
  sign, and run cycle. See `Makefile` at the repo root for what each target
  actually does; these are thin pass-throughs, not reimplementations.

  `:exclude [agent test]` — both names shadow rarely-used `clojure.core`
  vars (`core/agent`, the concurrency primitive; `core/test`, which runs a
  var's `:test` metadata function). The Makefile targets are named `agent`
  and `test`, so those are the names worth keeping here."
  (:refer-clojure :exclude [agent test])
  (:require [clojure.string :as str]
            [console.shell :as sh]))

(defn build
  "`make build` — debug build of bsdkrun, then codesign (macOS only)."
  []
  (sh/sh ["make" "build"]))

(defn release
  "`make release` — release build of bsdkrun, then codesign. Use this for
  anything you actually run."
  []
  (sh/sh ["make" "release"]))

(defn sign
  "`make sign` — (re)codesign the debug binaries with the hypervisor
  entitlement. A no-op on Linux."
  []
  (sh/sh ["make" "sign"]))

(defn sign-release
  "`make sign-release` — (re)codesign the release binaries."
  []
  (sh/sh ["make" "sign-release"]))

(defn web
  "`make web` — build the web SPA into web/dist, which build.rs embeds into
  the bsdkrun binary for `bsdkrun ui`. Run before `release` if you changed
  web/."
  []
  (sh/sh ["make" "web"]))

(defn daemon
  "`make daemon` — release build of bsdkrun-daemon + bsdkrun-supervisor,
  then codesign."
  []
  (sh/sh ["make" "daemon"]))

(defn agent
  "`make agent` — cross-compile the in-guest exec agent for every
  (os, arch) combination `make agent-linux`/`agent-freebsd`/`agent-netbsd`
  cover. Needs `cargo zigbuild` and, for FreeBSD, a nightly toolchain with
  `rust-src`."
  []
  (sh/sh ["make" "agent"]))

(defn agent-linux [] (sh/sh ["make" "agent-linux"]))
(defn agent-freebsd [] (sh/sh ["make" "agent-freebsd"]))
(defn agent-netbsd [] (sh/sh ["make" "agent-netbsd"]))

(defn run
  "`make run ARGS=...` — build (+sign) then run the debug binary, forwarding
  `args`. E.g. `(run \"ps\" \"--all\")`."
  [& args]
  (sh/sh (cond-> ["make" "run"]
           (seq args) (conj (str "ARGS=" (str/join " " args))))))

(defn test
  "`make test` (= `make e2e`) — build, then boot the FreeBSD image under a
  PTY and assert the beastie loader menu appears. Needs libkrun + a
  hypervisor (KVM/HVF) on the host."
  []
  (sh/sh ["make" "test"]))

(defn clean
  "`make clean` — `cargo clean`."
  []
  (sh/sh ["make" "clean"]))
