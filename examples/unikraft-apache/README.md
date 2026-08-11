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
<!DOCTYPE html>
<html>
<head><title>Apache on Unikraft</title></head>
<body>
<h1>Hello from Apache on Unikraft!</h1>
</body>
</html>
```

## Status

**Both architectures work.** arm64 boots, DHCPs an address, the start script
execs apache2, and `GET /` answers over the forwarded port:

```console
$ curl http://127.0.0.1:18083/
<!DOCTYPE html>
<html>
<head><title>Apache on Unikraft</title></head>
<body>
<h1>Hello from Apache on Unikraft!</h1>
</body>
</html>
```

Verified with `--mem 512`, guest port `8080` forwarded, repeated requests
against the same running machine. x86_64 is green on
`.github/workflows/e2e-unikraft-examples.yml`, after the `libgcc_s.so.1` fix
below.

**Getting arm64 there needed a real kernel patch — entry 26 in
`../../library/unikraft-base/patches/apply.sh`.** apache2 always writes a PID
file on startup, and doing so calls `chmod()`, traced in upstream's
`server/log.c`, `ap_log_pid()`:

```c
temp_fname = apr_pstrcat(p, fname, ".XXXXXX", NULL);
rv = apr_file_mktemp(&pid_file, temp_fname, ...);          /* succeeds -- the
                                                                ".ccjkCj" file
                                                                in the log line
                                                                below is this */
...
perms = APR_UREAD | APR_UWRITE | APR_GREAD | APR_WREAD;
if (((rv = apr_file_perms_set(temp_fname, perms)) != APR_SUCCESS
       && rv != APR_ENOTIMPL)                                /* <-- failed here */
    || (rv = apr_file_write_full(...)) != APR_SUCCESS
    || (rv = apr_file_close(pid_file)) != APR_SUCCESS
    || (rv = apr_file_rename(temp_fname, fname, p)) != APR_SUCCESS) {
    ap_log_error(..., APLOGNO(10231) "%s: Failed creating pid file %s", ...);
    exit(1);
}
```

Before the patch, this is what boot looked like -- apache2 got as far as:

```
[core:warn] (22)Invalid argument: AH00076: Failed to enable APR_TCP_DEFER_ACCEPT
[core:error] (38)Function not implemented: AH10231: apache2: Failed creating pid file /var/run/apache2/apache2.pid.ccjkCj
```

and then `exit(1)`'d. (The `APR_TCP_DEFER_ACCEPT` warning is unrelated and
harmless -- `TCP_DEFER_ACCEPT` is a Linux `setsockopt()` this guest's lwip
does not support, and Apache only warns and moves on.)

**Root cause: arm64 Linux has no raw `chmod` syscall at all**, and Unikraft's
syscall shim never implemented its replacement. arm64's ABI dropped every
"legacy" path syscall x86_64 still carries; glibc papers over that for
`chmod()` the exact same way it does for `open`, `mkdir`, and `unlink` --
`chmod(path, mode)` becomes `fchmodat(AT_FDCWD, path, mode, 0)`. vfscore
(`lib/vfscore`) already had working `openat`/`mkdirat`/`unlinkat`, and its
`chmod` `UK_SYSCALL_R_DEFINE` was itself a complete, correct implementation --
`sys_chmod()` → `vn_setmode()` → ramfs's `ramfs_setattr()` sets the mode bits
and returns success -- but nothing on arm64 could ever reach it, because
`chmod`'s syscall number does not exist there. Only `fchmodat` was missing:
never declared as a provided syscall, no handler defined, so every call fell
straight through to `ENOSYS`. Confirmed with a syscall trace before the fix:

```
openat(dirfd:4, "/var/run/apache2/apache2.pid.CsOhld", O_RDONLY|O_CREAT|O_EXCL|0x2) = fd:4
fchmodat(0xffffffffffffffda, 0x100086f090, ...) = Function not implemented (-38)
```

`../unikraft-postgres/README.md` already noted the same underlying gap for a
Unix-domain socket bind (*"Binding one ends in `chmod()`, which the guest
does not implement"*) and worked around it instead of fixing it, because
postgres didn't need the feature at all. Apache's PID-file write is not
optional the same way -- `ap_log_pid()` runs unconditionally on every
startup, `-X` included, with no config directive that skips it -- so this
port implements `fchmodat` in `library/unikraft-base/patches/apply.sh`
(patch 26) instead of working around it. It is a generic fix, not
apache-specific: any arm64 program calling `chmod()` hit this identically.
`AT_SYMLINK_NOFOLLOW` is not handled -- Linux's own `fchmodat` rejects that
flag too, so this isn't a narrower contract than upstream, just an
unimplemented corner nothing here exercises.

**x86_64 needed one more thing: `libgcc_s.so.1`, missing from the rootfs.**
`ldd` of `apache2` and its four loaded modules never lists it -- nothing has
it as a `DT_NEEDED` entry. glibc's NPTL `dlopen()`s it lazily, the first
time something needs stack unwinding (`pthread_exit()` among others), so its
absence only showed up at runtime, on CI:

```
libgcc_s.so.1 must be installed for pthread_exit to work
```

right after apache2 got past `APR_TCP_DEFER_ACCEPT` and the expected
`getpwuid` line below, and right before the guest exited. The Dockerfile now
copies it unconditionally alongside the `ldd`-resolved set.

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
