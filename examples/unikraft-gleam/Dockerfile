# The root filesystem for the Gleam unikernel: an Erlang shipment, the Erlang
# emulator, and the OTP applications the shipment actually depends on.
#
# Both stages use the same image, so the compiler and the emulator agree on
# their OTP version by construction. Architecture is handled the same way as in
# ../unikraft-elixir, and for the same reasons:
#
#   1. The compile stage runs on $BUILDPLATFORM. `gleam export erlang-shipment`
#      emits nothing but .beam bytecode and .app files, all of which are
#      architecture-independent, so building it under emulation would be waste.
#   2. The runtime stage runs on $TARGETPLATFORM and is the only source of
#      binaries. It resolves the shared libraries with `ldd` rather than naming
#      them, which is the only way one Dockerfile serves both architectures.
ARG GLEAM_VERSION=1.18.0

FROM --platform=$BUILDPLATFORM ghcr.io/gleam-lang/gleam:v${GLEAM_VERSION}-erlang AS build

WORKDIR /src

# manifest.toml pins every transitive dependency, so `deps download` is
# reproducible and worth its own layer.
COPY gleam.toml manifest.toml ./
RUN gleam deps download

COPY src ./src
RUN gleam export erlang-shipment
# -> /src/build/erlang-shipment/<app>/ebin/*.{beam,app}

# No --platform here on purpose: a stage without one is already built for
# $TARGETPLATFORM, and saying so explicitly is what buildx warns about.
FROM ghcr.io/gleam-lang/gleam:v${GLEAM_VERSION}-erlang AS runtime

# The shipment's layout -- one directory per application, each with an ebin/ --
# is exactly what ERL_LIBS expects, so /srv on ERL_LIBS replaces the `-pa
# "$BASE"/*/ebin` that the generated entrypoint.sh passes. That matters: the
# glob expands to twenty-odd arguments, and this one has to survive as a kernel
# command line.
COPY --from=build /src/build/erlang-shipment /rootfs/srv

RUN set -eu; \
    ERL=/usr/local/lib/erlang; \
    ERTS=$(basename "$ERL"/erts-*); \
    mkdir -p "/rootfs/erl/$ERTS/bin" /rootfs/erl/lib /rootfs/root /rootfs/tmp; \
    \
    # The emulator, plus the two helper programs ERTS execs by absolute path
    # out of -bindir. beam.smp alone is tens of megabytes, and nothing else in
    # erts-*/bin is reachable from a shipment that neither compiles code nor
    # uses distribution.
    for prog in beam.smp erl_child_setup inet_gethost; do \
        cp -a "$ERL/$ERTS/bin/$prog" "/rootfs/erl/$ERTS/bin/$prog"; \
    done; \
    # A version-independent alias, so the -bindir on the kernel command line
    # survives a bump of the base image's OTP.
    ln -s "$ERTS" /rootfs/erl/erts; \
    \
    # With no -boot on the command line the emulator falls back to
    # $ROOTDIR/bin/start.boot, and dies with
    #
    #   Runtime terminating during boot ({'cannot get bootfile','/erl/bin/start.boot'})
    #
    # if it is not there. ../unikraft-elixir never needs these because a mix
    # release names its own boot script; a Gleam shipment has none, so the
    # stock ones come across. They are a few kilobytes each.
    mkdir -p /rootfs/erl/bin; \
    cp -a "$ERL"/bin/*.boot /rootfs/erl/bin/; \
    \
    # Which OTP applications to carry. A shipment has no .rel file to read the
    # answer out of -- that is a release concept, and Gleam does not build one
    # -- but every .app file names its own dependencies, so walking that graph
    # to a fixed point gives the same answer. It reaches crypto and ssl through
    # glisten and gleam_crypto, and public_key and asn1 through ssl.
    deps_of() { \
        tr -d ' \n' < "$1" \
          | sed -n 's/.*{applications,\[\([^]]*\)\].*/\1/p' \
          | tr ',' '\n'; \
        # The .app file is a single Erlang term, so the newlines are stripped
        # to match it -- which leaves sed with an unterminated last line, and
        # one application's last dependency glued to the next one's first.
        echo; \
    }; \
    queue=$(for f in /rootfs/srv/*/ebin/*.app; do deps_of "$f"; done | sort -u); \
    seen=' '; \
    while [ -n "$queue" ]; do \
        next=''; \
        for app in $queue; do \
            case "$seen" in *" $app "*) continue ;; esac; \
            seen="$seen$app "; \
            d=$(echo "$ERL/lib/$app"-*); \
            [ -d "$d" ] || continue; \
            cp -a "$d" /rootfs/erl/lib/; \
            next="$next $(deps_of "$d/ebin/$app.app")"; \
        done; \
        queue=$(echo "$next" | tr ' ' '\n' | sort -u); \
    done; \
    echo "OTP applications carried:"; ls /rootfs/erl/lib; \
    \
    # Sources, headers and documentation ship inside the OTP applications and
    # are dead weight in a unikernel, where the image is resident twice.
    find /rootfs/erl -type d \
        \( -name src -o -name doc -o -name examples -o -name man -o -name include \) \
        -prune -exec rm -rf {} +; \
    \
    # Resolve the shared libraries of everything executable that survived: the
    # emulator, the helper programs, and the NIFs under lib/*/priv, which are
    # dlopen()ed at runtime and drag in OpenSSL. `ldd` reports whatever this
    # architecture actually needs, including the correct dynamic loader.
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
