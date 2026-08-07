# Homebrew formula for bsdkrund, the bsdkrun daemon.
#
# Copy this into the tap repo as `Formula/bsdkrund.rb`
# (github.com/tsirysndr/homebrew-tap) — Homebrew installs from the tap, not
# from here. It lives in this repo so it is versioned with the thing it ships.
#
# Installs prebuilt binaries from the GitHub release that `release-daemon.yml`
# publishes. To cut a new version: bump `version`, then read each `sha256` from
# the `.sha256` sidecar published beside its tarball, e.g.
#
#   curl -sL https://github.com/tsirysndr/bsdkrun/releases/download/v0.6.0/bsdkrund-aarch64-apple-darwin.tar.gz.sha256
class Bsdkrund < Formula
  desc "Token-authenticated gRPC + GraphQL daemon for driving bsdkrun remotely"
  homepage "https://github.com/tsirysndr/bsdkrun"
  version "0.6.0"
  license "MIT"

  # The daemon owns no VM logic: it resolves the `bsdkrun` binary and runs it as
  # a subprocess. Without the CLI it starts and then fails every request, so
  # this is a hard runtime dependency rather than a suggestion.
  depends_on "tsirysndr/tap/bsdkrun"

  on_macos do
    # macOS is Apple Silicon only, matching the CLI — bsdkrun needs an arm64
    # host for libkrun, so an Intel daemon would have no CLI to drive.
    on_arm do
      url "https://github.com/tsirysndr/bsdkrun/releases/download/v#{version}/bsdkrund-aarch64-apple-darwin.tar.gz"
      sha256 "REPLACE_WITH_SHA256_FROM_THE_RELEASE"
    end
  end

  on_linux do
    on_intel do
      # Statically linked against musl, so it runs on any distro.
      url "https://github.com/tsirysndr/bsdkrun/releases/download/v#{version}/bsdkrund-x86_64-unknown-linux-musl.tar.gz"
      sha256 "REPLACE_WITH_SHA256_FROM_THE_RELEASE"
    end
  end

  def install
    # macOS arm64 and Linux amd64 only. Homebrew would otherwise fail on the
    # missing `url` with nothing explaining why, and the release does publish a
    # linux-arm64 tarball for manual download.
    on_macos do
      odie "bsdkrund is only built for Apple Silicon on macOS" unless Hardware::CPU.arm?
    end
    on_linux do
      odie "bsdkrund is only built for x86_64 on Linux" unless Hardware::CPU.intel?
    end

    bin.install "bsdkrund"
  end

  # `brew services start bsdkrund`.
  #
  # Loopback by default: the daemon hands out full VM and shell access on the
  # host, so exposing it to a network stays a deliberate act. To reach it from
  # another machine, edit the plist/unit to bind 0.0.0.0 and put TLS or a
  # tunnel in front — see `bsdkrund --help`.
  service do
    run [opt_bin/"bsdkrund", "--bind", "127.0.0.1:50051", "--graphql-bind", "127.0.0.1:50052"]
    keep_alive true
    log_path var/"log/bsdkrund.log"
    error_log_path var/"log/bsdkrund.log"
    # A service manager's PATH is minimal, and the daemon has to be able to find
    # `bsdkrun` (and the tools it shells out to: gvproxy, curl, tar).
    environment_variables PATH: std_service_path_env
  end

  def caveats
    <<~EOS
      bsdkrund needs an access token. Set one so it survives restarts:

        BSDKRUN_TOKEN=$(openssl rand -hex 32)
        brew services start bsdkrund

      Started without one it generates a token and prints it on stdout, which
      under `brew services` means the log rather than your terminal:

        grep -m1 -A2 "access token" #{var}/log/bsdkrund.log

      Point a client at it:

        export BSDKRUN_HOST=http://127.0.0.1:50051   # CLI / desktop app (gRPC)
        export BSDKRUN_TOKEN=<token>

      The web UI uses the GraphQL port instead (http://127.0.0.1:50052/graphql),
      and `bsdkrun ui` serves that UI.

      It listens on loopback. Binding it to a network gives anyone who reaches
      it full VM and shell access on this host, so use TLS (--tls-cert/--tls-key)
      or an encrypted tunnel if you do.
    EOS
  end

  test do
    assert_match "bsdkrund", shell_output("#{bin}/bsdkrund --version")

    # It must refuse to start when it cannot find a bsdkrun to drive — that is
    # the failure the dependency above exists to prevent, and a formula that
    # installed a daemon which silently did nothing would be worse than one
    # that failed here.
    output = shell_output("#{bin}/bsdkrund --bsdkrun /nonexistent/bsdkrun 2>&1", 1)
    assert_match "bsdkrun", output
  end
end
