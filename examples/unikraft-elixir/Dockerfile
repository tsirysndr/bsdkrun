# The root filesystem for the Elixir unikernel: a mix release, the Erlang
# emulator, and the OTP applications the release's boot script actually
# resolves -- nothing else.
#
# Architecture is handled in two places, and neither of them is a hardcoded
# path (upstream's Dockerfile lists `/lib/x86_64-linux-gnu/...` by hand, which
# cannot be made to work on arm64: the directory *and* the dynamic loader's
# filename both differ):
#
#   1. The compile stage runs on $BUILDPLATFORM. Everything it produces is
#      Erlang bytecode, which is architecture-independent, so building it under
#      QEMU for the target would be pure waste. `mix release` is given
#      `include_erts: false` precisely so that no host-arch binary can leak
#      into the release.
#   2. The runtime stage runs on $TARGETPLATFORM and is the *only* source of
#      binaries. It resolves the shared libraries with `ldd` rather than naming
#      them, the same way the other Unikraft examples here do.
ARG ELIXIR_VERSION=1.16.2

FROM --platform=$BUILDPLATFORM elixir:${ELIXIR_VERSION}-slim AS build

WORKDIR /src
ENV MIX_ENV=prod HEX_HOME=/tmp/hex MIX_HOME=/tmp/mix

# The slim image ships no CA bundle, so Hex falls back to unverified TLS and
# warns on every download. The checksums in mix.lock are still checked, but an
# unverified fetch is not something to leave in place.
RUN apt-get update -qq \
    && apt-get install -y -qq --no-install-recommends ca-certificates \
    && rm -rf /var/lib/apt/lists/*

COPY mix.exs mix.lock ./
RUN mix local.hex --force && mix local.rebar --force && mix deps.get

COPY lib ./lib
RUN mix release --overwrite

# No --platform here on purpose: a stage without one is already built for
# $TARGETPLATFORM, and saying so explicitly is what buildx warns about.
FROM elixir:${ELIXIR_VERSION}-slim AS runtime

# The release goes to /srv rather than somewhere under /usr/src: its path ends
# up in the kernel command line three times (-boot, -boot_var, -config), and
# that command line is already long. See the README.
COPY --from=build /src/_build/prod/rel/server /rootfs/srv

RUN set -eu; \
    ERL=/usr/local/lib/erlang; \
    ERTS=$(basename "$ERL"/erts-*); \
    mkdir -p "/rootfs/erl/$ERTS/bin" /rootfs/erl/lib /rootfs/root /rootfs/tmp; \
    \
    # The emulator, plus the two helper programs ERTS execs by absolute path
    # out of -bindir. Nothing else in erts-*/bin is reachable from a release
    # that neither compiles code nor uses distribution, and beam.smp alone is
    # 37 MiB, so the rest is not worth carrying.
    for prog in beam.smp erl_child_setup inet_gethost; do \
        cp -a "$ERL/$ERTS/bin/$prog" "/rootfs/erl/$ERTS/bin/$prog"; \
    done; \
    # A version-independent alias, so that the -bindir on the kernel command
    # line does not have to be edited whenever the base image's OTP moves.
    ln -s "$ERTS" /rootfs/erl/erts; \
    \
    # Which OTP applications to carry is not a guess: `mix release` writes the
    # answer into start.script, as the $ROOT-relative code path of every
    # application the boot script loads. (The release's own applications --
    # elixir, plug, cowboy and friends -- are under $RELEASE_LIB instead, and
    # came across with /srv above.)
    grep -o '"\$ROOT/lib/[^"]*/ebin"' /rootfs/srv/releases/*/start.script \
      | sed 's|.*/lib/\(.*\)/ebin"|\1|' \
      | sort -u \
      | while read -r app; do \
            cp -a "$ERL/lib/$app" /rootfs/erl/lib/; \
        done; \
    \
    # Sources, headers and documentation ship inside the OTP applications and
    # are dead weight in a unikernel, where the image is resident twice (see
    # the README's note on memory).
    find /rootfs/erl -type d \
        \( -name src -o -name doc -o -name examples -o -name man -o -name include \) \
        -prune -exec rm -rf {} +; \
    \
    # Resolve the shared libraries of everything executable that survived --
    # the emulator, the helper programs, and the NIFs under lib/*/priv, which
    # are dlopen()ed at runtime and drag in OpenSSL. `ldd` reports whatever
    # this architecture actually needs, including the correct dynamic loader.
    find /rootfs/erl -type f \( -perm -u+x -o -name '*.so' \) -print \
      | while read -r f; do ldd "$f" 2>/dev/null || true; done \
      | grep -oE '/[^ ()]+' \
      | sort -u \
      | while read -r lib; do \
            mkdir -p "/rootfs$(dirname "$lib")"; \
            cp -L "$lib" "/rootfs$lib"; \
        done

# /etc/resolv.conf, /etc/hosts and /etc/hostname are generated at boot by the
# ELF loader (CONFIG_APPELFLOADER_AUTOGEN_* in the Kraftfile), so this image
# deliberately ships no /etc.
FROM scratch

COPY --from=runtime /rootfs/ /
