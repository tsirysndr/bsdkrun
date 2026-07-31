# bsdkrun — build helpers.
#
# libkrun on macOS requires the Hypervisor.framework entitlement, and every
# `cargo build` strips the codesignature, so we re-sign after each build.

BIN_DEBUG   := target/debug/bsdkrun
BIN_RELEASE := target/release/bsdkrun
ENTITLEMENTS := bsdkrun.entitlements

.PHONY: build release sign sign-release run test e2e clean

build:
	cargo build
	@$(MAKE) sign

release:
	cargo build --release
	codesign --entitlements $(ENTITLEMENTS) --force -s - $(BIN_RELEASE)

# Sign the debug binary with the hypervisor entitlement.
sign:
	codesign --entitlements $(ENTITLEMENTS) --force -s - $(BIN_DEBUG)

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
