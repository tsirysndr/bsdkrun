# mariadb:11.4 is the current LTS and is published for both amd64 and arm64.
FROM mariadb:11.4 AS build

# A statically linked shell, for argv hygiene rather than for scripting:
# libkrun appends its own words (earlycon=..., tsi_hijack, a bare `--`) to the
# end of the kernel command line, past the `--` stop sequence, so they arrive
# in the application's argv. mysqld ignores them; mariadbd does not --
#
#   mariadbd: Too many arguments (first extra is 'earlycon=pl011,mmio32,...')
#   [ERROR] Aborting
#
# -- so it is booted as `sh /start.sh` instead: the junk becomes the script's
# positional parameters and the script exec()s mariadbd with a clean argv, an
# execve() rather than a fork (CONFIG_APPELFLOADER_MULTIPROCESS in the
# Kraftfile). Same pattern as ../unikraft-redis and ../unikraft-mongodb.
FROM busybox:stable-musl AS shell

# The data directory is initialised *here*, when the image is built, instead of
# by docker-entrypoint.sh when the container starts. Upstream's port ships the
# whole entrypoint plus bash, gosu, awk and a dozen coreutils to run it -- a
# process tree, and a unikernel runs one program: there is no fork() to give
# it. Baking the datadir moves all of that to build time, so the guest boots
# straight into mariadbd with no shell in the image at all.
#
# --no-defaults: the image's own configuration sets user=mysql and paths that
# only make sense with the entrypoint; the guest has exactly one user, root.
#
# --auth-root-authentication-method=normal: the default is unix_socket
# authentication, which would leave root unable to log in over TCP -- and TCP
# through the forwarded port is the only way into the guest.
RUN set -eux; \
    rm -rf /var/lib/mysql; \
    mkdir -p /var/lib/mysql; \
    mariadb-install-db --no-defaults --user=root --datadir=/var/lib/mysql \
        --auth-root-authentication-method=normal --skip-test-db

# Give root a password and let it connect from anywhere; connections through
# the forwarded port arrive from the gvproxy gateway address, so 'root'@'%' is
# the account that matters.
#
# A real server on a unix socket, not `mariadbd --bootstrap`: bootstrap mode
# implies skip-grant-tables (account statements fail with ERROR 1290), and --
# the part that actually bit -- it leaves Aria's transaction log unclean, so
# the guest's first boot began with "Aria engine: starting recovery ...
# transactions to roll back: 1" and hung there. A normal start and a clean
# `mariadb-admin shutdown` checkpoint both engines, and the guest boots a
# datadir with no recovery to do. Same approach as ../unikraft-mysql.
RUN set -eux; \
    mariadbd --no-defaults --user=root --datadir=/var/lib/mysql \
             --skip-networking --socket=/tmp/bootstrap.sock & \
    for _ in $(seq 60); do \
        mariadb --socket=/tmp/bootstrap.sock -u root -e "SELECT 1" \
            >/dev/null 2>&1 && break; \
        sleep 1; \
    done; \
    mariadb --socket=/tmp/bootstrap.sock -u root -e " \
        CREATE USER 'root'@'%' IDENTIFIED BY 'unikraft'; \
        GRANT ALL PRIVILEGES ON *.* TO 'root'@'%' WITH GRANT OPTION; \
        SET PASSWORD FOR 'root'@'localhost' = PASSWORD('unikraft');"; \
    mariadb-admin --socket=/tmp/bootstrap.sock -u root -punikraft shutdown; \
    for _ in $(seq 30); do [ -S /tmp/bootstrap.sock ] || break; sleep 1; done; \
    test ! -S /tmp/bootstrap.sock

# Reset Aria to a virgin state: zerofill the Aria system tables (strips the
# transaction ids and LSNs that tie them to this server's log -- the
# documented procedure for moving Aria tables between servers) and delete the
# log and its control file, which a fresh server recreates at startup.
#
# This is load-bearing, not tidying. Booted with the log present, mariadbd
# under Unikraft enters "Aria engine: starting recovery ... transactions to
# roll back: 1" and spins forever in userspace -- on a datadir that the same
# binary on plain Linux opens with no recovery at all (verified byte-for-byte
# identical via the guest's own syscall trace). Until that divergence is
# understood, ship a datadir that gives Aria nothing to recover.
RUN set -eux; \
    aria_chk --zerofill /var/lib/mysql/mysql/*.MAI; \
    rm -f /var/lib/mysql/aria_log.* /var/lib/mysql/aria_log_control

# Assemble the root filesystem.
#
# The libraries are resolved with ldd rather than listed by hand: upstream
# lists /lib/x86_64-linux-gnu/... paths, which do not exist on arm64, and ldd
# also picks up anything a future mariadbd starts needing (liburing included --
# it is linked even though the guest cannot use it; see my.cnf).
#
# Out of /usr/share/mariadb (or /usr/share/mysql, depending on the package
# layout), only english/ and charsets/ are kept: the rest is other languages'
# errmsg.sys, and every megabyte here is paid for twice at boot (once embedded
# in the kernel image, once unpacked into ramfs). No plugins are copied: the
# engines this datadir uses (InnoDB, Aria) are built into mariadbd.
COPY --from=shell /bin/busybox /bin/busybox.static
RUN set -eux; \
    mkdir -p /rootfs/usr/sbin /rootfs/usr/share /rootfs/var/lib \
             /rootfs/bin /rootfs/tmp /rootfs/etc; \
    cp /bin/busybox.static /rootfs/bin/busybox; \
    ln -s busybox /rootfs/bin/sh; \
    cp /usr/sbin/mariadbd /rootfs/usr/sbin/mariadbd; \
    ldd /usr/sbin/mariadbd \
      | grep -oE '/[^ ()]+' \
      | sort -u \
      | while read -r lib; do \
            mkdir -p "/rootfs$(dirname "$lib")"; \
            cp -L "$lib" "/rootfs$lib"; \
        done; \
    SHARE=/usr/share/mysql; \
    [ -d /usr/share/mariadb ] && [ ! -L /usr/share/mariadb ] \
        && SHARE=/usr/share/mariadb; \
    mkdir -p "/rootfs$SHARE"; \
    cp -a "$SHARE/english" "$SHARE/charsets" "/rootfs$SHARE/"; \
    for s in /usr/share/mariadb /usr/share/mysql; do \
        [ -L "$s" ] && cp -a "$s" "/rootfs$s" || true; \
    done; \
    cp -a /var/lib/mysql /rootfs/var/lib/mysql; \
    chown -R 0:0 /rootfs/var; \
    chmod 1777 /rootfs/tmp

# The server's configuration lives in the image rather than on the command
# line. Everything in it is either a unikernel constraint or a size cut; see
# the comments there and the README.
COPY my.cnf /rootfs/etc/my.cnf
COPY start.sh /rootfs/start.sh

FROM scratch

COPY --from=build /rootfs/ /
