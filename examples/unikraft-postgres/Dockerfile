# PostgreSQL 16, built from source and *initialised* at image build time.
#
# Two things differ from upstream's unikraft-cloud/examples/postgres, and both
# come from the same fact: that image targets Unikraft Cloud's `base-compat`
# runtime, which has real processes, and this one targets stock Unikraft, which
# does not (see README.md).
#
#   1. No `wrapper.sh`. Upstream boots into a bash port of docker-library's
#      entrypoint, which runs `initdb`, starts a temporary server through
#      `pg_ctl`, creates the database with `psql` and then `exec`s the real
#      postgres. Every one of those steps is a separate process. Here `initdb`
#      runs in this Dockerfile instead, on the build machine, and the finished
#      data directory is baked into the image — so the guest starts at the
#      point upstream's wrapper finishes.
#   2. A much smaller build. ICU, readline, zlib, lz4, libxml and libxslt are
#      all configured out. They cost about 40 MiB of shared libraries and data,
#      and the root filesystem is resident *twice* in a Unikraft guest (embedded
#      in the kernel image and unpacked into ramfs), so the saving is doubled.
FROM alpine:3.20 AS pg-build

ARG PG_VERSION=16.4

RUN apk add --no-cache \
        bison \
        bzip2 \
        coreutils \
        flex \
        g++ \
        gcc \
        linux-headers \
        make \
        musl-dev \
        patch \
        perl \
        tar

ADD https://ftp.postgresql.org/pub/source/v${PG_VERSION}/postgresql-${PG_VERSION}.tar.bz2 /tmp/postgresql.tar.bz2
COPY allow-root.patch no-sysv-ipc.patch dsm-single-address-space.patch \
     exec-backend-posix-spawn.patch exec-backend-self-path.patch /tmp/

# `--without-icu` is the one flag that is not just about size: PostgreSQL 16
# builds against ICU by default, and ICU's collation lookup wants a locale
# database this image has no room for. The C locale is what initdb is told to
# use below anyway.
RUN set -eux; \
    mkdir -p /usr/src; \
    tar -xjf /tmp/postgresql.tar.bz2 -C /usr/src; \
    cd "/usr/src/postgresql-${PG_VERSION}"; \
    patch -p1 < /tmp/allow-root.patch; \
    patch -p1 < /tmp/no-sysv-ipc.patch; \
    patch -p1 < /tmp/dsm-single-address-space.patch; \
    patch -p1 < /tmp/exec-backend-posix-spawn.patch; \
    patch -p1 < /tmp/exec-backend-self-path.patch; \
    export CFLAGS="-Os -Wl,--strip-all"; \
    # -DEXEC_BACKEND is the whole ballgame. PostgreSQL normally creates every
    # child with a bare fork() and lets it inherit the postmaster's memory;
    # Unikraft has no fork() and never will, being single-address-space. In an
    # EXEC_BACKEND build -- what PostgreSQL uses on Windows, and supports on
    # Unix for testing -- the postmaster instead forks and immediately execs
    # `postgres --forkchild`, handing the child its state through a file. That
    # is a fork-then-exec, which is exactly what Unikraft's vfork() supports;
    # exec-backend-posix-spawn.patch turns the pair into the one call that is
    # safe to make. Every fork_process() call site in the tree is inside
    # `#else /* !EXEC_BACKEND */`, so this leaves none behind.
    export CPPFLAGS="-DEXEC_BACKEND"; \
    ./configure \
        --enable-option-checking=fatal \
        --prefix=/usr/local \
        --with-pgport=5432 \
        --disable-rpath \
        --without-icu \
        --without-readline \
        --without-zlib \
        --without-lz4 \
        --without-zstd \
    ; \
    make -j "$(nproc)"; \
    make install

# Initialise the cluster here rather than in the guest.
#
# `initdb` is itself a multi-process program -- it runs `postgres --boot` and
# `postgres --single` as children to load the catalogue -- so it could not run
# in the unikernel even if there were a shell to invoke it from. Running it at
# build time is not a workaround for a missing feature so much as the natural
# shape for an immutable image: the cluster is part of the artefact.
#
# --auth=trust because there is no network listener to authenticate against;
# single-user mode connects through neither TCP nor a socket. Set a password
# and use scram-sha-256 if you ever get a postmaster running (see README.md).
ENV PGDATA=/var/lib/postgresql/data
RUN set -eux; \
    mkdir -p "$PGDATA"; \
    chmod 700 "$PGDATA"; \
    initdb -D "$PGDATA" \
        --username=postgres \
        --auth=trust \
        --encoding=UTF8 \
        --locale=C \
        --no-sync; \
    # Trim the defaults to something a 2 GiB guest is comfortable with. The
    # process-count settings are not tuning: every one of these workers is a
    # fork, so leaving them on only produces a failed clone() during startup.
    { \
        echo; \
        echo "# --- bsdkrun/unikraft ---"; \
        echo "shared_buffers = 32MB"; \
        # The guest's stack is 2 MiB (CONFIG_STACK_SIZE_PAGE_ORDER in the
        # Kraftfile) and PostgreSQL insists on 512 KiB of headroom below
        # whatever this is set to, so the default 2MB would not start.
        echo "max_stack_depth = 1MB"; \
        echo "max_connections = 10"; \
        # Background workers stay off: they are real processes and each one is
        # another spawn to go wrong. The aux processes the postmaster always
        # starts (checkpointer, background writer, WAL writer) are not covered
        # by these settings and do run.
        echo "max_worker_processes = 0"; \
        echo "max_parallel_workers = 0"; \
        echo "max_parallel_workers_per_gather = 0"; \
        echo "max_wal_senders = 0"; \
        echo "wal_level = minimal"; \
        echo "autovacuum = off"; \
        echo "logging_collector = off"; \
        # Deliberately *not* shared_memory_type=mmap: with the default (sysv)
        # the whole segment goes through no-sysv-ipc.patch and comes back as
        # one anonymous mapping that every process already shares, which is
        # what lets an exec'd child pick it up again. mmap mode would split it
        # into an anonymous block plus a SysV shim and only the shim address is
        # handed down.
        echo "dynamic_shared_memory_type = mmap"; \
        echo "fsync = off"; \
        echo "listen_addresses = '0.0.0.0'"; \
        # No Unix-domain socket: binding one ends in chmod(), which the guest
        # does not implement --
        #   LOG:  could not set permissions of file "/tmp/.s.PGSQL.5432":
        #         Function not implemented
        #   FATAL:  could not create any Unix-domain sockets
        # Nothing in the guest can connect over one anyway; clients arrive over
        # the forwarded TCP port.
        echo "unix_socket_directories = ''"; \
    } >> "$PGDATA/postgresql.conf"; \
    # Let clients in from anywhere, without a password. initdb's default
    # pg_hba.conf covers only 127.0.0.1 and ::1, but a client forwarded by
    # gvproxy arrives as the gateway (192.168.127.1):
    #
    #   FATAL:  no pg_hba.conf entry for host "192.168.127.1", user "postgres"
    #
    # `trust` is what upstream's wrapper.sh defaults to as well when no
    # password is set. The guest is reachable only through ports bsdkrun
    # explicitly forwards.
    echo "host all all all trust" >> "$PGDATA/pg_hba.conf"

# Assemble the root filesystem: the server, its data, and nothing else.
#
# Libraries are resolved with `ldd` rather than listed. Upstream hardcodes
# /lib/ld-musl-x86_64.so.1; on arm64 the musl loader is ld-musl-aarch64.so.1 --
# a different filename, so no substitution fixes it in place.
RUN set -eux; \
    mkdir -p /rootfs/usr/local/bin /rootfs/usr/local/lib /rootfs/usr/local/share \
             /rootfs/var/lib /rootfs/var/run/postgresql /rootfs/tmp /rootfs/etc; \
    cp /usr/local/bin/postgres /rootfs/usr/local/bin/postgres; \
    # A shell, because the postmaster needs one. Before it execs a child it
    # version-checks the binary it is about to run, and does it with popen():
    #
    #   ERR: [libukbinfmt] /bin/sh: Loader not found
    #   popen failure: Exec format error
    #   FATAL:  postgres: could not locate matching postgres executable
    #
    # busybox is 800 KiB, is already linked against the musl in this image, and
    # -- the part that matters -- is a PIE, which is what execve() requires on a
    # single-address-space kernel: the loader has to place it somewhere other
    # than where the caller is already running.
    mkdir -p /rootfs/bin; \
    cp /bin/busybox /rootfs/bin/busybox; \
    ln -s busybox /rootfs/bin/sh; \
    ldd /usr/local/bin/postgres /bin/busybox \
      | grep -oE '/[^ ()]+' \
      | sort -u \
      | while read -r lib; do \
            mkdir -p "/rootfs$(dirname "$lib")"; \
            cp -L "$lib" "/rootfs$lib"; \
        done; \
    # share/postgresql holds the timezone database and the .sample configs;
    # lib/postgresql holds plpgsql.so, which template1 loads on demand.
    cp -a /usr/local/share/postgresql /rootfs/usr/local/share/; \
    cp -a /usr/local/lib/postgresql /rootfs/usr/local/lib/; \
    cp -a /var/lib/postgresql /rootfs/var/lib/; \
    rm -rf /rootfs/usr/local/share/postgresql/man /rootfs/usr/local/share/doc; \
    chmod 1777 /rootfs/tmp; \
    # PostgreSQL asks *musl* who it is running as -- getpwuid(geteuid()) -- and
    # musl answers out of /etc/passwd, not out of Unikraft's posix-user (which
    # would say "root"). In single-user mode that name has to be an existing
    # role, because there is no -U to override it with; the cluster above was
    # created with `--username=postgres`, so uid 0 is called postgres here.
    echo 'postgres:x:0:0:postgres:/var/lib/postgresql:/bin/sh' > /rootfs/etc/passwd; \
    echo 'postgres:x:0:' > /rootfs/etc/group

# /etc/hosts, /etc/resolv.conf and /etc/hostname are generated at boot by the
# ELF loader (CONFIG_APPELFLOADER_AUTOGEN_*), so they are deliberately absent.
#
# Note what the COPY below does to hard links: BuildKit deduplicates identical
# files as it writes them, so /usr/local/share/postgresql/timezone comes out
# the other side with 245 of them even though the stage above has none. Unikraft
# has no link(), so build.sh breaks them again after `docker export` -- which is
# the only place it can be done and stay done. See build.sh and README.md.
FROM scratch

COPY --from=pg-build /rootfs/ /
