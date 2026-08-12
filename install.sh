#!/bin/sh
# Install bsdkrun — plus bsdkrun-supervisor, bsdkrund and libkrun — from its
# GitHub releases:
#
#   curl -fsSL https://raw.githubusercontent.com/tsirysndr/bsdkrun/main/install.sh | sh
#
# Environment variables:
#
#   BSDKRUN_VERSION=v0.8.1      pin a release (default: latest). Also accepted
#                               as the first argument: `sh -s -- v0.8.1`.
#   BSDKRUN_INSTALL=~/.bsdkrun  install root; binaries land in its bin/
#   BSDKRUN_SKIP_GVPROXY=1      skip the gvproxy (guest networking) download
#
# Every archive is verified against its .sha256 sidecar. On Linux the CLI
# archive bundles libkrun/libkrunfw (rpath'd to $ORIGIN), so the extracted
# tree is self-contained — which is why everything is unpacked into a
# directory of its own instead of copying one binary into /usr/local/bin:
# the shared objects have to stay beside the binary for its rpath to find
# them. macOS gets libkrun from Homebrew (our fork), installed here when
# brew is available. bsdkrund and bsdkrun-supervisor land in the same bin/,
# which is also how they find each other: the daemon looks for the
# supervisor beside itself before consulting PATH.

set -eu

REPO="tsirysndr/bsdkrun"
# gvproxy provides the guest's user-mode networking. bsdkrun boots fine
# without it (just no NIC), so its download is best-effort, like the npm
# package's postinstall.
GVPROXY_REPO="containers/gvisor-tap-vsock"

say() { printf '%s\n' "$*"; }
err() { printf 'install.sh: %s\n' "$*" >&2; exit 1; }

# Neon teal, but only where it renders as color: a terminal, with color not
# opted out of (NO_COLOR is the convention, TERM=dumb the older signal).
banner() {
    if [ -t 1 ] && [ -z "${NO_COLOR:-}" ] && [ "${TERM:-}" != "dumb" ]; then
        printf '\033[1;38;2;0;255;204m'
    fi
    # Quoted heredoc: the art's backslashes must reach the terminal literally.
    cat <<'EOF'
    __             ____
   / /_  _________/ / /_________  ______
  / __ \/ ___/ __  / //_/ ___/ / / / __ \
 / /_/ (__  ) /_/ / ,< / /  / /_/ / / / /
/_.___/____/\__,_/_/|_/_/   \__,_/_/ /_/
EOF
    if [ -t 1 ] && [ -z "${NO_COLOR:-}" ] && [ "${TERM:-}" != "dumb" ]; then
        printf '\033[0m'
    fi
    say ""
}

main() {
    banner
    version="${1:-${BSDKRUN_VERSION:-}}"
    root="${BSDKRUN_INSTALL:-$HOME/.bsdkrun}"
    bin_dir="$root/bin"

    command -v curl >/dev/null 2>&1 || err "curl is required"
    command -v tar >/dev/null 2>&1 || err "tar is required"

    # The daemon's Linux triple is musl, not gnu: bsdkrund is deliberately a
    # static binary that runs on any distro, and its release workflow builds
    # it that way. Same OS/arch, different asset suffix.
    os="$(uname -s)"
    arch="$(uname -m)"
    case "$os $arch" in
    "Darwin arm64")
        triple="aarch64-apple-darwin"
        daemon_triple="aarch64-apple-darwin"
        gvproxy_asset="gvproxy-darwin"
        ;;
    "Linux x86_64")
        triple="x86_64-unknown-linux-gnu"
        daemon_triple="x86_64-unknown-linux-musl"
        gvproxy_asset="gvproxy-linux-amd64"
        ;;
    "Linux aarch64" | "Linux arm64")
        triple="aarch64-unknown-linux-gnu"
        daemon_triple="aarch64-unknown-linux-musl"
        gvproxy_asset="gvproxy-linux-arm64"
        ;;
    *)
        err "unsupported platform: $os/$arch
bsdkrun ships prebuilt binaries for macOS/arm64 (Apple Silicon), Linux/x86_64
and Linux/aarch64. On macOS, Intel is not supported."
        ;;
    esac

    if [ -n "$version" ]; then
        # Accept both `0.8.1` and `v0.8.1` — tags carry the `v`.
        case "$version" in v*) ;; *) version="v$version" ;; esac
        base="https://github.com/$REPO/releases/download/$version"
    else
        base="https://github.com/$REPO/releases/latest/download"
    fi

    tmp="$(mktemp -d)"
    trap 'rm -rf "$tmp"' EXIT
    mkdir -p "$bin_dir"

    # The CLI bundle. On Linux it already carries bsdkrun-supervisor and the
    # bundled libkrun/libkrunfw *.so files, all rpath'd to $ORIGIN.
    fetch_tarball "bsdkrun-$triple.tar.gz" required
    [ -e "$bin_dir/bsdkrun" ] || err "archive did not contain a 'bsdkrun' binary"
    say "installed bsdkrun ${version:-latest} -> $bin_dir/bsdkrun"

    # bsdkrun-supervisor: the daemon hands anything that boots a machine to
    # it. The Linux CLI bundle already includes it; macOS ships it separately.
    if [ ! -e "$bin_dir/bsdkrun-supervisor" ]; then
        fetch_tarball "bsdkrun-supervisor-$triple.tar.gz" optional &&
            say "installed bsdkrun-supervisor -> $bin_dir/bsdkrun-supervisor"
    fi

    # bsdkrund: the gRPC/GraphQL daemon. It finds the supervisor beside
    # itself, which is exactly where the lines above put it.
    if fetch_tarball "bsdkrund-$daemon_triple.tar.gz" optional; then
        say "installed bsdkrund -> $bin_dir/bsdkrund"
    fi

    install_gvproxy
    install_libkrun_macos

    case ":$PATH:" in
    *":$bin_dir:"*) ;;
    *)
        say ""
        say "add bsdkrun to your PATH:"
        say "  export PATH=\"$bin_dir:\$PATH\""
        ;;
    esac
}

# Download a release tarball, verify it, and extract it into $bin_dir.
# `required` failures abort the install; `optional` ones (a component the
# pinned release predates) warn and return non-zero so the caller can skip
# its success message.
fetch_tarball() {
    tb_asset="$1" tb_mode="$2"
    say "downloading $base/$tb_asset"
    if ! curl -fSL --progress-bar -o "$tmp/$tb_asset" "$base/$tb_asset"; then
        [ "$tb_mode" = required ] && err \
            "download failed. If you pinned a version, does the release exist?
  https://github.com/$REPO/releases"
        say "warning: $tb_asset is not in this release — skipping"
        return 1
    fi
    verify_checksum "$tmp/$tb_asset" "$base/$tb_asset.sha256" "$tb_asset"
    tar -xzf "$tmp/$tb_asset" -C "$bin_dir"
    return 0
}

# Verify against the .sha256 sidecar each release publishes. A missing
# sidecar (older releases) warns and continues; a mismatch fails hard.
verify_checksum() {
    archive="$1" sidecar_url="$2" name="$3"
    expected="$(curl -fsSL "$sidecar_url" 2>/dev/null | awk '{print tolower($1)}')" || expected=""
    if [ -z "$expected" ]; then
        say "warning: no checksum sidecar for $name — skipping verification"
        return 0
    fi
    if command -v sha256sum >/dev/null 2>&1; then
        actual="$(sha256sum "$archive" | awk '{print $1}')"
    elif command -v shasum >/dev/null 2>&1; then
        actual="$(shasum -a 256 "$archive" | awk '{print $1}')"
    else
        say "warning: no sha256sum/shasum on this system — skipping verification"
        return 0
    fi
    [ "$expected" = "$actual" ] || err "checksum mismatch for $name:
  expected $expected
  actual   $actual"
    say "checksum OK ($(printf '%.12s' "$actual")…)"
}

install_gvproxy() {
    if [ -n "${BSDKRUN_SKIP_GVPROXY:-}" ]; then
        say "BSDKRUN_SKIP_GVPROXY set — skipping gvproxy download"
        return 0
    fi
    if [ -e "$bin_dir/gvproxy" ] || command -v gvproxy >/dev/null 2>&1; then
        return 0
    fi
    url="https://github.com/$GVPROXY_REPO/releases/latest/download/$gvproxy_asset"
    say "downloading $url"
    # gvproxy assets are raw executables, no archive. Never fail the install
    # over this — the guest still boots, just without a NIC.
    if curl -fSL --progress-bar -o "$bin_dir/gvproxy" "$url"; then
        chmod +x "$bin_dir/gvproxy"
        say "installed gvproxy -> $bin_dir/gvproxy"
    else
        rm -f "$bin_dir/gvproxy"
        say "warning: could not download gvproxy. Guest networking will be"
        say "unavailable until you install it (e.g. \`brew install gvproxy\`)."
    fi
}

# libkrun is the one piece this script cannot place itself on macOS: the
# binary links it from Homebrew's prefix, and it must be OUR fork (PVH boot,
# virtio-fs fixes), not upstream's. On Linux there is nothing to do — the CLI
# bundle already carries libkrun.so/libkrunfw.so beside the binary.
install_libkrun_macos() {
    [ "$os" = Darwin ] || return 0
    [ ! -e /opt/homebrew/lib/libkrun.dylib ] || return 0
    if command -v brew >/dev/null 2>&1; then
        say "installing libkrun (tsirysndr/tap/libkrun) via Homebrew..."
        brew install tsirysndr/tap/libkrun || {
            say "warning: brew install failed. bsdkrun needs libkrun to run VMs:"
            say "  brew install tsirysndr/tap/libkrun"
        }
    else
        say ""
        say "note: bsdkrun on macOS needs our libkrun fork. Install Homebrew, then:"
        say "  brew install tsirysndr/tap/libkrun"
    fi
}

main "$@"
