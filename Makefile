# bsdkrun — build helpers.
#
# On macOS, libkrun needs the Hypervisor.framework entitlement and every
# `cargo build` strips the codesignature, so we re-sign after each build. On
# Linux (KVM) there's nothing to sign — the sign steps are no-ops there.

BIN_DEBUG    := target/debug/bsdkrun
BIN_RELEASE  := target/release/bsdkrun
# The daemon links the same engine and boots machines through its own binary,
# so it needs the hypervisor entitlement exactly as the CLI does.
DAEMON_DEBUG   := target/debug/bsdkrund
DAEMON_RELEASE := target/release/bsdkrund
ENTITLEMENTS := bsdkrun.entitlements
UNAME_S      := $(shell uname -s)

# In-guest exec agent: a separate crate cross-compiled to static per-(os,arch)
# guest binaries and published as GitHub release assets. bsdkrun downloads +
# caches the matching one at runtime (Linux auto-injected; FreeBSD/NetBSD are
# for the user to copy into a running BSD guest). Not tracked in git.
AGENT_DIR := core/src/agent-bin

.PHONY: build release sign sign-release run test e2e clean web daemon \
        sign-daemon agent agent-linux agent-freebsd agent-netbsd

build:
	cargo build
	@$(MAKE) sign

# Release build (+sign). Use this for anything you actually run.
release:
	cargo build --release
	@$(MAKE) sign-release

# Sign with the hypervisor entitlement (macOS only; a no-op elsewhere).
sign:
ifeq ($(UNAME_S),Darwin)
	codesign --entitlements $(ENTITLEMENTS) --force -s - $(BIN_DEBUG)
	@[ -f $(DAEMON_DEBUG) ] && codesign --entitlements $(ENTITLEMENTS) --force -s - $(DAEMON_DEBUG) || true
endif

sign-release:
ifeq ($(UNAME_S),Darwin)
	codesign --entitlements $(ENTITLEMENTS) --force -s - $(BIN_RELEASE)
	@[ -f $(DAEMON_RELEASE) ] && codesign --entitlements $(ENTITLEMENTS) --force -s - $(DAEMON_RELEASE) || true
endif

# Sign just the daemon (macOS only; a no-op elsewhere).
sign-daemon:
ifeq ($(UNAME_S),Darwin)
	codesign --entitlements $(ENTITLEMENTS) --force -s - $(DAEMON_RELEASE)
endif

# --- web UI ----------------------------------------------------------------
#
# Builds the SPA in web/ into web/dist, which build.rs embeds into the bsdkrun
# binary for `bsdkrun ui`. Run this BEFORE `make release` — cargo picks up
# whatever is in web/dist at compile time, and build.rs writes a placeholder
# page there when the real bundle is missing, so a checkout without node still
# compiles.
web:
	cd web && (bun install || npm install) && (bun run build || npm run build)

# --- daemon ----------------------------------------------------------------
#
# Standalone crate (own workspace): the gRPC + GraphQL server.
daemon:
	cargo build --release -p bsdkrun-daemon
	@$(MAKE) sign-daemon

# --- guest agents (release assets) -----------------------------------------
#
# Linux + FreeBSD build for both arches with `cargo zigbuild` (zig ships their
# libc/sysroot). NetBSD has no zig libc and is built natively in CI, so it's a
# local no-op. FreeBSD needs nightly + rust-src (std isn't distributed):
#   rustup toolchain install nightly && rustup component add rust-src --toolchain nightly
agent: agent-linux agent-freebsd agent-netbsd

agent-linux:
	@mkdir -p $(AGENT_DIR)
	cd agent && cargo zigbuild --release --target aarch64-unknown-linux-musl
	cp agent/target/aarch64-unknown-linux-musl/release/bsdkrun-agent $(AGENT_DIR)/bsdkrun-agent.linux-aarch64
	cd agent && cargo zigbuild --release --target x86_64-unknown-linux-musl
	cp agent/target/x86_64-unknown-linux-musl/release/bsdkrun-agent $(AGENT_DIR)/bsdkrun-agent.linux-x86_64

agent-freebsd:
	@mkdir -p $(AGENT_DIR)
	cd agent && cargo +nightly zigbuild --release --target aarch64-unknown-freebsd -Z build-std=std,panic_abort
	cp agent/target/aarch64-unknown-freebsd/release/bsdkrun-agent $(AGENT_DIR)/bsdkrun-agent.freebsd-aarch64
	cd agent && cargo +nightly zigbuild --release --target x86_64-unknown-freebsd -Z build-std=std,panic_abort
	cp agent/target/x86_64-unknown-freebsd/release/bsdkrun-agent $(AGENT_DIR)/bsdkrun-agent.freebsd-x86_64

agent-netbsd:
	@echo "note: netbsd agents can't cross-compile via zig; built natively in CI."

# Convenience: build (+sign) then run, forwarding args via ARGS=...
run: build
	$(BIN_DEBUG) $(ARGS)

# End-to-end boot test: build, then boot the FreeBSD image under a PTY and
# assert the beastie loader menu appears. Override DISK=/FIRMWARE=/etc. as env.
test: e2e
e2e: build
	BSDKRUN_BIN=$(BIN_DEBUG) tests/e2e_boot.sh

clean:
	cargo clean
