# bsdkrun — build helpers.
#
# libkrun on macOS requires the Hypervisor.framework entitlement, and every
# `cargo build` strips the codesignature, so we re-sign after each build.

BIN_DEBUG   := target/debug/bsdkrun
BIN_RELEASE := target/release/bsdkrun
ENTITLEMENTS := bsdkrun.entitlements

.PHONY: build release sign sign-release run clean

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

clean:
	cargo clean
