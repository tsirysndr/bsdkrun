# bsdkrun — build helpers.
#
# libkrun on macOS requires the Hypervisor.framework entitlement, and every
# `cargo build` strips the codesignature, so we re-sign after each build.

BIN_DEBUG   := target/debug/bsdkrun
BIN_RELEASE := target/release/bsdkrun
ENTITLEMENTS := bsdkrun.entitlements

# In-guest exec agent: a separate crate cross-compiled to static aarch64 guest
# binaries and published as GitHub release assets. bsdkrun downloads + caches the
# matching one at runtime (Linux is auto-injected; FreeBSD/NetBSD are for the user
# to copy into a running BSD guest). These targets produce the release assets —
# the host build no longer embeds them. Not tracked in git.
AGENT_DIR         := src/agent-bin
AGENT_BIN         := $(AGENT_DIR)/bsdkrun-agent.linux-aarch64
AGENT_BIN_FREEBSD := $(AGENT_DIR)/bsdkrun-agent.freebsd-aarch64
AGENT_BIN_NETBSD  := $(AGENT_DIR)/bsdkrun-agent.netbsd-aarch64

.PHONY: build release sign sign-release run test e2e agent agent-linux agent-freebsd agent-netbsd clean

build:
	cargo build
	@$(MAKE) sign

# Cross-compile the release-asset guest agents. Linux + FreeBSD build with
# `cargo zigbuild` (zig ships their libc/sysroot); NetBSD has no zig libc and is
# built natively in CI, so it's a local no-op.
agent: agent-linux agent-freebsd agent-netbsd

# Linux: stable + musl (zig provides the sysroot).
agent-linux:
	cd agent && cargo zigbuild --release --target aarch64-unknown-linux-musl
	@mkdir -p $(AGENT_DIR)
	cp agent/target/aarch64-unknown-linux-musl/release/bsdkrun-agent $(AGENT_BIN)

# FreeBSD: needs nightly + rust-src (std isn't distributed for this target):
#   rustup toolchain install nightly && rustup component add rust-src --toolchain nightly
agent-freebsd:
	cd agent && cargo +nightly zigbuild --release --target aarch64-unknown-freebsd \
		-Z build-std=std,panic_abort
	@mkdir -p $(AGENT_DIR)
	cp agent/target/aarch64-unknown-freebsd/release/bsdkrun-agent $(AGENT_BIN_FREEBSD)

# NetBSD: zig has no NetBSD libc, so it can't be cross-compiled from macOS. CI
# builds it natively inside a NetBSD VM (.github/workflows/release-netbsd-agent.yml);
# locally this is a no-op so `make agent` still succeeds.
agent-netbsd:
	@echo "note: aarch64-unknown-netbsd can't cross-compile via zig; built natively in CI."

# Release build (+sign). Use this for anything you actually run.
release:
	cargo build --release
	@$(MAKE) sign-release

# Sign the debug binary with the hypervisor entitlement.
sign:
	codesign --entitlements $(ENTITLEMENTS) --force -s - $(BIN_DEBUG)

# Sign the release binary with the hypervisor entitlement.
sign-release:
	codesign --entitlements $(ENTITLEMENTS) --force -s - $(BIN_RELEASE)

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
