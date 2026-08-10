# unikraft-apache

Apache httpd (Debian's `apache2` package), running as a Unikraft unikernel.
There is no upstream `unikraft-cloud/examples` port to follow here (unlike
`../unikraft-php` and `../unikraft-redis`) -- this is built from scratch by
trimming Debian's `apache2` package down to a single foreground process
serving one static `index.html`, closely following the patterns already
established by this repo's other examples.

```sh
./build.sh                    # host arch; or: ./build.sh x86_64
bsdkrun unikraft . --mem 512 --port 18083:8080 \
  --cmdline "elfloader -- /bin/sh /start.sh"
```

```console
$ curl http://127.0.0.1:18083/
Hello from Apache on Unikraft!
```

(That's the expectation upstream servers here document. See **Status** below
-- this one does not actually get there yet.)

## Status

**arm64 boots, DHCPs, and starts apache2 -- then apache2 exits before it ever
listens.** The kernel comes up, `en1` gets an address from gvproxy, the start
script `exec`s `apache2 -X -f /etc/apache2/apache2.conf`, and apache2 begins
its own startup (module init, socket setup). It gets as far as:

```
[core:warn] (22)Invalid argument: AH00076: Failed to enable APR_TCP_DEFER_ACCEPT
[core:warn] (22)Invalid argument: AH00076: Failed to enable APR_TCP_DEFER_ACCEPT
[core:error] (38)Function not implemented: AH10231: apache2: Failed creating pid file /var/run/apache2/apache2.pid.ccjkCj
```

and then `exit(1)`s. The `APR_TCP_DEFER_ACCEPT` warnings are harmless --
`TCP_DEFER_ACCEPT` is a Linux `setsockopt()` this guest's lwip does not
support, and Apache only warns and moves on. The `AH10231` line is fatal, and
it is not this port's bug to fix.

**Root cause: `apache2` always writes a PID file, every time it starts, and
doing so calls `chmod()`.** Traced in upstream's `server/log.c`,
`ap_log_pid()`:

```c
temp_fname = apr_pstrcat(p, fname, ".XXXXXX", NULL);
rv = apr_file_mktemp(&pid_file, temp_fname, ...);          /* succeeds -- the
                                                                ".ccjkCj" file
                                                                in the log line
                                                                above is this */
...
perms = APR_UREAD | APR_UWRITE | APR_GREAD | APR_WREAD;
if (((rv = apr_file_perms_set(temp_fname, perms)) != APR_SUCCESS
       && rv != APR_ENOTIMPL)                                /* <-- fails here */
    || (rv = apr_file_write_full(...)) != APR_SUCCESS
    || (rv = apr_file_close(pid_file)) != APR_SUCCESS
    || (rv = apr_file_rename(temp_fname, fname, p)) != APR_SUCCESS) {
    ap_log_error(..., APLOGNO(10231) "%s: Failed creating pid file %s", ...);
    exit(1);
}
```

`apr_file_perms_set()` is a `chmod()` on the freshly created temp file. That
call comes back `ENOSYS` ("Function not implemented"), which APR turns into
an OS-specific error code -- not the symbolic `APR_ENOTIMPL` the `&&` clause
was written to tolerate -- so the whole condition is true and Apache exits
before `apr_file_write_full`, `apr_file_close` or `apr_file_rename` are ever
reached. `write_full`/`close`/`rename` are never the problem; `chmod()` is.

**This is not a new discovery, and not something to patch here.**
`../unikraft-postgres/README.md` already documents that this repo's Unikraft
base has no `chmod()`: *"No Unix-domain socket. Binding one ends in
`chmod()`, which the guest does not implement (`could not create any
Unix-domain sockets`)."* postgres worked around it by not needing the
feature at all (no Unix socket, TCP only). Apache's PID-file write is not
optional the same way -- `ap_log_pid()` runs unconditionally from `main()` on
every startup, `-X` (single-process debug mode) included, and there is no
`PidFile`-adjacent directive that skips it; the compiled-in default path is
used even with no `PidFile` line in the config at all. Working around it
would mean either patching the kernel's syscall shim to implement `chmod()`
(a real, general-purpose gap -- out of scope for a from-scratch application
port, per this example's brief) or patching/recompiling Apache's own `log.c`
to treat any `apr_file_perms_set()` failure as tolerable (a source rebuild,
which this port deliberately avoids -- see below). Neither was attempted.

x86_64 has never been run.

## What this trims from stock Apache

**Everything not needed to serve one static file.** `apt-get install -y
--no-install-recommends apache2` pulls in `mpm_event`, `authz_host`,
`authz_core`, `authn_core`, `auth_basic`, `authn_file`, `authz_user`,
`alias`, `dir`, `autoindex`, `env`, `mime`, `negotiation`, `setenvif`,
`filter`, `deflate`, `status`, `reqtimeout`, and Debian's default vhost,
security, and CGI config snippets. `apache2.conf` here `LoadModule`s exactly
four: `mpm_event`, `authz_core` (for `Require all granted`), `mime`, and
`dir` (for `DirectoryIndex`). No CGI, no userdir, no autoindex, no status
page, no auth, no compression, no content negotiation.

**`-X`, not `apachectl start`.** Every Apache MPM (prefork, worker, event)
normally forks at least one child process to do the actual serving; the
parent only supervises. Unikraft has no general-purpose `fork()`. `-X`
("debug mode") is Apache's own single-process, non-detaching mode -- normally
meant for running httpd under a debugger -- and it is the one mode that never
forks: verified locally (`ps aux` under a plain Debian container shows
exactly one `apache2` process running with `-X`, versus a parent plus worker
children without it). That made the redis-style fork problem moot here
without needing `../unikraft-redis`'s `CONFIG_APPELFLOADER_MULTIPROCESS`
exec-only workaround for *this* reason -- though this Kraftfile still turns
it on, for the argv trampoline below.

**No `User`/`Group` directive -- deliberately absent, not just left at
`root`.** Debian's `apache2` binary is compiled *without*
`-DBIG_SECURITY_HOLE`, so an explicit `User root` fails config parsing
outright:

```
Apache has not been designed to serve pages while running as root...
add -DBIG_SECURITY_HOLE to the CFLAGS env variable and then rebuild the server
```

The obvious fix, `User www-data`, parses fine but calls `setuid(33)` at
startup -- and this repo's `CONFIG_LIBPOSIX_USER` is one fixed identity
(uid/gid 0, "root"), so that call was expected to fail. Tested by leaving the
`User`/`Group` directives out entirely instead: `unixd`'s configured uid then
stays `-1`, which sidesteps *both* problems -- the "running as root" check
only fires on an *explicit* uid/gid of `0`, and no `setuid()` is attempted at
all -- at the cost of one harmless startup line, `[unixd:alert] AH02155:
getpwuid: couldn't determine user name from uid 4294967295`. Verified working
end-to-end (curl got a 200) in a plain Debian container before this ever
touched the unikernel.

**`Mutex pthread default`, not APR's own `posixsem` default.** A single
process needs no cross-process mutex; `pthread` mutexes are plain futexes,
which this repo's kconfig already provides (`CONFIG_LIBPOSIX_FUTEX`).

**`ErrorLog "/dev/stdout"`, not `/dev/stderr`.** `library/unikraft-base`'s
devfs registers exactly one console device node -- `/dev/stdout` (see
`lib/devfs/stdout.c`: *"One function for stderr and stdout"*) -- there is no
`/dev/stderr` node to open. `../unikraft-nginx`-style `error_log stderr;`
would fail to open its log target here; `/dev/stdout` is the node that
actually exists and reaches `bsdkrun logs`.

**One static `index.html`, one trimmed `mime.types`.** Debian's own
`/etc/mime.types` is close to a thousand lines; `TypesConfig` here points at
a seven-line file covering the shipped `index.html` and a handful of common
static types.

## The argv trampoline

Same problem as `../unikraft-redis`: libkrun appends its own words
(`earlycon=...`, `tsi_hijack`, a bare `--`) past the kernel command line's
`--` stop sequence, and they land in the application's `argv`. Verified
directly: `apache2 -X -f /etc/apache2/apache2-min.conf 'earlycon=pl011,...'
--` prints Apache's `Usage:` banner and exits nonzero -- its `getopt`-based
parser has no tolerance for unrecognized words, same as redis-server. `boot
sh /start.sh` instead makes the junk the script's positional parameters,
which nothing reads, and `start.sh` `exec`s apache2 with a clean argv --
`execve()`, no fork, enabled by `CONFIG_APPELFLOADER_MULTIPROCESS` in the
Kraftfile. The shell is the statically linked busybox (~1 MiB), same as
`../unikraft-redis`.

## Layout

| file            | role                                                              |
|-----------------|-------------------------------------------------------------------|
| `Dockerfile`    | rootfs: `apache2`, four modules, their libraries (via `ldd`), busybox |
| `apache2.conf`  | ServerRoot/Mutex/modules/DocumentRoot, commented                  |
| `mime.types`    | trimmed TypesConfig target, not Debian's ~1000-line original      |
| `index.html`    | the one page served                                                |
| `start.sh`      | argv filter: soaks up libkrun's junk, `exec`s apache2 with `-X`   |
| `Kraftfile`     | the from-source base runtime + elfloader + MULTIPROCESS for exec  |
| `build.sh`      | two-phase build; see `../unikraft-postgres/build.sh`               |
