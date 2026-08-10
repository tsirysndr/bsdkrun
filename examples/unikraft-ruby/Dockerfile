# ruby:3.2-bookworm is published for both amd64 and arm64; upstream pins
# --platform=linux/x86_64, which is exactly what this port removes.
FROM ruby:3.2-bookworm AS build

# Assemble the rootfs by *resolving* dependencies rather than listing them.
# Upstream enumerates /lib/x86_64-linux-gnu/... paths, which do not exist on
# arm64; asking ldd keeps this correct on both.
#
# The stdlib's native extensions (socket.so, and everything else under
# /usr/local/lib/ruby) are dlopen()ed, so ldd of the ruby binary alone would
# miss their libraries -- libz for zlib, libssl for openssl, libyaml for
# psych. Resolving every bundled .so picks them all up; libruby.so itself
# lands in place the same way, because ldd reports it with its full path.
RUN set -eu; \
    mkdir -p /rootfs/usr/bin /rootfs/src /rootfs/tmp; \
    cp /usr/local/bin/ruby /rootfs/usr/bin/ruby; \
    mkdir -p /rootfs/usr/local/lib; \
    cp -a /usr/local/lib/ruby /rootfs/usr/local/lib/ruby; \
    { ldd /usr/local/bin/ruby; \
      find /usr/local/lib/ruby -name '*.so' -exec ldd {} \; 2>/dev/null \
      || true; } \
      | grep -oE '/[^ ()]+' \
      | sort -u \
      | while read -r lib; do \
            mkdir -p "/rootfs$(dirname "$lib")"; \
            cp -L "$lib" "/rootfs$lib"; \
        done; \
    chmod 1777 /rootfs/tmp

# The single-threaded TCPServer HTTP loop, verbatim from upstream.
COPY server.rb /rootfs/src/server.rb

# /etc/resolv.conf, /etc/hosts and /etc/hostname are generated at boot by
# app-elfloader (CONFIG_APPELFLOADER_AUTOGEN_* in the Kraftfile).
FROM scratch

COPY --from=build /rootfs/ /
