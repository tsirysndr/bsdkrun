# The default root filesystem for the base image: a single static binary that
# explains how to supply your own. Matches unikraft/catalog's library/base.
#
# No `--platform` here on purpose: buildx is invoked with `--platform` for the
# target architecture (see build.sh), so this stage runs emulated and its gcc
# is natively the target's. That is what makes the binary come out as the right
# architecture without any cross-compiler plumbing.
FROM gcc:13.2.0 AS fallback

COPY ./fallback.c /fallback.c

RUN set -xe; \
    gcc \
      -fPIC \
      -g \
      -o /fallback \
      -O3 \
      -static-pie \
      -Wall \
      -Wextra \
      -Wno-unused \
      -Wno-unused-parameter \
      /fallback.c

FROM scratch

COPY --from=fallback /fallback /fallback
