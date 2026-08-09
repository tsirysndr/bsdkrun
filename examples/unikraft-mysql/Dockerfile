# The Oracle Linux flavoured `mysql:8.0` rather than upstream's
# `mysql:8.0-debian`: the Debian variant is published for amd64 only, and this
# example has to build for arm64 as well.
FROM mysql:8.0 AS build

# The data directory is initialised *here*, when the image is built, instead of
# by docker-entrypoint.sh when the container starts. Upstream's entrypoint is a
# bash script that runs mysqld twice (once for `--initialize`, once for real),
# shells out to gosu, mysqladmin, awk and sed, and backgrounds a server to
# bootstrap the user table. That is a process tree, and a unikernel runs one
# program: there is no fork() to give it.
#
# Baking the datadir moves all of that to build time, so the guest boots
# straight into mysqld with no shell in the image at all. It also removes the
# first-boot initialisation from the measured boot -- the point of running a
# database in a unikernel.
#
# --no-defaults: the base image's /etc/my.cnf sets `user=mysql`, which wins over
# a later --user=root on the command line ("Ignoring user change to 'root'
# because the user was set to 'mysql' earlier") and leaves the datadir owned by
# uid 27. The guest has exactly one user, root; see the Kraftfile's
# CONFIG_LIBPOSIX_USER_*.
RUN set -eux; \
    rm -rf /var/lib/mysql; \
    mkdir -p /var/lib/mysql; \
    mysqld --no-defaults --user=root --datadir=/var/lib/mysql \
           --initialize-insecure \
           --innodb-redo-log-capacity=8388608

# Give root a password and let it connect from anywhere. --initialize-insecure
# only creates root@localhost with an empty password, and a database nobody can
# reach from off-box is not much of an example.
#
# This is the one place a server has to run at build time: the user table is
# SQL, not files that can be dropped in. mysqld is started on a unix socket
# only, handed the statements through --init-file, and shut down again.
RUN set -eux; \
    printf '%s\n' \
      "ALTER USER 'root'@'localhost' IDENTIFIED WITH mysql_native_password BY 'unikraft';" \
      "CREATE USER IF NOT EXISTS 'root'@'%' IDENTIFIED WITH mysql_native_password BY 'unikraft';" \
      "GRANT ALL PRIVILEGES ON *.* TO 'root'@'%' WITH GRANT OPTION;" \
      > /tmp/bootstrap.sql; \
    mysqld --no-defaults --user=root --datadir=/var/lib/mysql \
           --innodb-redo-log-capacity=8388608 \
           --skip-networking --socket=/tmp/bootstrap.sock \
           --init-file=/tmp/bootstrap.sql & \
    for _ in $(seq 60); do \
        mysqladmin --socket=/tmp/bootstrap.sock -u root -punikraft ping >/dev/null 2>&1 && break; \
        sleep 1; \
    done; \
    mysqladmin --socket=/tmp/bootstrap.sock -u root -punikraft shutdown; \
    for _ in $(seq 30); do [ -S /tmp/bootstrap.sock ] || break; sleep 1; done; \
    test ! -S /tmp/bootstrap.sock

# mysql_native_password rather than the default caching_sha2_password: the
# latter's first handshake from a new client is done over TLS, or over a
# plaintext channel only after an RSA public-key exchange. Both work, but they
# turn "does the database answer?" into a TLS question, and the client-side
# incantation (`--get-server-public-key`) is easy to get wrong. Nothing here
# depends on it; drop the `IDENTIFIED WITH` clauses to use the modern default.

# Assemble the root filesystem.
#
# The libraries are resolved with ldd rather than listed by hand (upstream lists
# /lib/x86_64-linux-gnu/... paths, which do not exist on arm64). Note that one
# entry, libprotobuf-lite, is found through mysqld's RPATH as
# /usr/sbin/../lib64/mysql/private/... -- an absolute path with a `..` in it, so
# copying it verbatim lands it where the RPATH will look for it in the guest.
RUN set -eux; \
    mkdir -p /rootfs/usr/sbin /rootfs/var/lib /rootfs/var/run/mysqld \
             /rootfs/var/lib/mysql-files /rootfs/tmp /rootfs/etc; \
    cp /usr/sbin/mysqld /rootfs/usr/sbin/mysqld; \
    ldd /usr/sbin/mysqld \
      | grep -oE '/[^ ()]+' \
      | sort -u \
      | while read -r lib; do \
            mkdir -p "/rootfs$(dirname "$lib")"; \
            cp -L "$lib" "/rootfs$lib"; \
        done; \
    mkdir -p /rootfs/usr/share/mysql-8.0 /rootfs/usr/lib64/mysql/plugin; \
    cp -a /usr/share/mysql-8.0/english /rootfs/usr/share/mysql-8.0/english; \
    cp -a /usr/share/mysql-8.0/charsets /rootfs/usr/share/mysql-8.0/charsets; \
    cp -a /usr/lib64/mysql/plugin/component_reference_cache.so \
          /rootfs/usr/lib64/mysql/plugin/; \
    cp -a /var/lib/mysql /rootfs/var/lib/mysql; \
    rm -f /rootfs/var/lib/mysql/*.pem; \
    chown -R 0:0 /rootfs/var; \
    chmod 1777 /rootfs/tmp

# Only `english/` and `charsets/` out of the 11 MiB /usr/share/mysql-8.0: the
# rest is other languages' errmsg.sys plus the .sql scripts the entrypoint uses,
# and every megabyte here is paid for twice at boot (once in the kernel image,
# once unpacked into ramfs).
#
# One plugin out of the 14 MiB plugin directory: component_reference_cache is
# registered in mysql.component by --initialize, so mysqld tries to dlopen() it
# on every start and complains loudly when it is missing. The rest (keyring,
# auth_pam, the clone plugin...) is not referenced by this datadir.
#
# Not copied: /usr/lib64/mysql/private/icudt77l, 4.2 MiB of ICU tables that only
# REGEXP_* needs. Their absence costs one startup warning ("Missing data
# directory for ICU regular expressions"). Add it if you want regexes.
#
# The *.pem are dropped for the same reason TLS is off below, and because a
# certificate baked into a public image is worse than no certificate: everyone
# who pulls it gets the same private key.
#
# /var/lib/mysql-files is empty and still required: mysqld realpath()s
# --secure-file-priv before it does anything else and aborts if it is missing.

# The server's configuration lives in the image rather than on the command line,
# because bsdkrun passes argv through --cmdline and a 20-word argv is unpleasant
# to type. Everything here is either a unikernel constraint or a size cut; see
# the README.
COPY my.cnf /rootfs/etc/my.cnf

FROM scratch

COPY --from=build /rootfs/ /
