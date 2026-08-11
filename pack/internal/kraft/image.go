package kraft

import (
	"context"
	"fmt"
	"io"
	"os"
	"os/exec"
	"path/filepath"
)

// builderDockerfile bakes everything buildContainer used to install fresh
// inside a --rm container on *every* build (apt packages, the kraftkit deb,
// rustup + its bare-metal targets) into an image instead. Building it is
// slow (network installs, same as before) but happens once per kraftVersion
// per host arch; every later `bsdkrun pack` run reuses the cached image and
// skips straight to kraftSteps.
const builderDockerfile = `FROM debian:bookworm

ARG KRAFT_VERSION
ARG HOST_DEB_ARCH
ARG CROSS_PKGS

RUN apt-get update -qq && \
	apt-get install -y -qq --no-install-recommends \
		build-essential libncurses-dev libyaml-dev flex bison git wget \
		unzip uuid-runtime python3 curl ca-certificates bc file patch \
		${CROSS_PKGS} && \
	rm -rf /var/lib/apt/lists/*

RUN curl -sSfLo /tmp/kraft.deb \
		https://github.com/unikraft/kraftkit/releases/download/v${KRAFT_VERSION}/kraftkit_${KRAFT_VERSION}_linux_${HOST_DEB_ARCH}.deb && \
	dpkg -i /tmp/kraft.deb && \
	rm /tmp/kraft.deb

# rustup rather than Debian's rustc: bookworm ships 1.63, too old for the
# bare-metal targets app-elfloader-rs (the loader kraft build compiles) needs.
RUN curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs \
		| sh -s -- -y --no-modify-path --profile minimal --default-toolchain stable && \
	/root/.cargo/bin/rustup target add aarch64-unknown-none-softfloat x86_64-unknown-none
ENV PATH="/root/.cargo/bin:${PATH}"
`

// builderImageTag names the cached image for a given host arch. It's keyed
// on kraftVersion so bumping that constant naturally invalidates the cache
// instead of silently reusing a stale kraftkit install.
func builderImageTag(hostDebArch string) string {
	return fmt.Sprintf("bsdkrun-pack-builder:%s-%s", kraftVersion, hostDebArch)
}

// crossPackages is the cross-compiler apt packages the *other* arch needs —
// baked into the image unconditionally (rather than decided per-run from
// --target) since a given host's arch, and so its "other" arch, never
// changes between runs.
func crossPackages(hostDebArch string) string {
	switch hostDebArch {
	case "arm64":
		return "gcc-x86-64-linux-gnu binutils-x86-64-linux-gnu"
	case "amd64":
		return "gcc-aarch64-linux-gnu binutils-aarch64-linux-gnu"
	default:
		return ""
	}
}

// ensureBuilderImage returns the tag of a builder image with the toolchain
// buildContainer's script needs, building it first if it isn't already
// cached locally. w receives `docker build`'s output on a cache miss (the
// only time it runs and takes real time); it's silent on a cache hit.
func ensureBuilderImage(ctx context.Context, hostDebArch string, w io.Writer) (string, error) {
	tag := builderImageTag(hostDebArch)
	if exec.CommandContext(ctx, "docker", "image", "inspect", tag).Run() == nil {
		return tag, nil
	}

	dir, err := os.MkdirTemp("", "bsdkrun-pack-builder-")
	if err != nil {
		return "", err
	}
	defer os.RemoveAll(dir)
	if err := os.WriteFile(filepath.Join(dir, "Dockerfile"), []byte(builderDockerfile), 0o644); err != nil {
		return "", err
	}

	cmd := exec.CommandContext(ctx, "docker", "build",
		"-t", tag,
		"--build-arg", "KRAFT_VERSION="+kraftVersion,
		"--build-arg", "HOST_DEB_ARCH="+hostDebArch,
		"--build-arg", "CROSS_PKGS="+crossPackages(hostDebArch),
		dir,
	)
	cmd.Stdout = w
	cmd.Stderr = w
	if err := cmd.Run(); err != nil {
		return "", fmt.Errorf("building pack builder image: %w", err)
	}
	return tag, nil
}
