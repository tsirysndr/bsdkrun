# ruby:3.2-bookworm is published for both amd64 and arm64; upstream pins
# nothing, but its library list is x86_64-only, which is what this port
# replaces with ldd below.
FROM ruby:3.2-bookworm AS build

# Rails 7.1, pinned: an unpinned `gem install rails` moves under the example's
# feet, and 7.2 raised the minimum Ruby past this image. 7.1 apps need no
# node -- the default is importmap -- so the build stays a pure-Ruby affair.
RUN gem install rails -v '~> 7.1.0'

# A stock application, generated at image build time exactly as upstream does
# it, then overlaid with the four files this example actually cares about
# (controller, view, routes, and a dev-environment tweak).
RUN rails new /app --skip-git
WORKDIR /app
RUN rails generate controller hello

COPY app/ /app/app/
COPY config/ /app/config/
COPY boot.rb /app/boot.rb

# Assemble the rootfs by *resolving* dependencies rather than listing them.
# Upstream enumerates /lib/x86_64-linux-gnu/... paths, which do not exist on
# arm64; asking ldd keeps this correct on both.
#
# Native extensions -- the stdlib's under /usr/local/lib/ruby and the gems'
# under /usr/local/bundle (sqlite3, nokogiri, ...) -- are dlopen()ed, so ldd
# of the ruby binary alone would miss their libraries. Resolving every bundled
# .so picks them all up; libruby.so itself lands in place the same way,
# because ldd reports it with its full path.
#
# /usr/share/zoneinfo comes along because ActiveSupport's tzinfo reads the
# system zoneinfo on Linux rather than shipping its own.
RUN set -eu; \
    mkdir -p /rootfs/usr/bin /rootfs/usr/local/lib /rootfs/usr/share \
             /rootfs/tmp; \
    cp /usr/local/bin/ruby /rootfs/usr/bin/ruby; \
    cp -a /usr/local/lib/ruby /rootfs/usr/local/lib/ruby; \
    cp -a /usr/local/bundle /rootfs/usr/local/bundle; \
    cp -a /usr/share/zoneinfo /rootfs/usr/share/zoneinfo; \
    cp -a /app /rootfs/app; \
    { ldd /usr/local/bin/ruby; \
      find /usr/local/lib/ruby /usr/local/bundle -name '*.so' \
           -exec ldd {} \; 2>/dev/null || true; } \
      | grep -oE '/[^ ()]+' \
      | sort -u \
      | while read -r lib; do \
            mkdir -p "/rootfs$(dirname "$lib")"; \
            cp -L "$lib" "/rootfs$lib"; \
        done; \
    chmod 1777 /rootfs/tmp

# /etc/resolv.conf, /etc/hosts and /etc/hostname are generated at boot by
# app-elfloader (CONFIG_APPELFLOADER_AUTOGEN_* in the Kraftfile).
FROM scratch

COPY --from=build /rootfs/ /
