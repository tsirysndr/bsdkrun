# unikraft-nginx

[nginx](https://nginx.org/) 1.25.3, serving static files, running as a
Unikraft unikernel. Ported from [`unikraft-cloud/examples`'s
`nginx`](https://github.com/unikraft-cloud/examples/tree/main/nginx) to build
for **arm64** as well as x86_64 and boot under bsdkrun.

```sh
./build.sh                    # host arch; or: ./build.sh x86_64
bsdkrun unikraft . --mem 512 --port 18082:8080 \
  --cmdline "elfloader -- /bin/sh /start.sh"
```

```console
$ curl http://127.0.0.1:18082/
<!DOCTYPE html>
<html>
...
<h1>Welcome to nginx!</h1>
...
```

## Status

**arm64 works.** The unikernel boots, DHCPs an address (`en1`, via gvproxy),
the start script execs nginx, and `GET /` answers over the forwarded port
with HTTP 200, a `Server: nginx/1.25.3` header, and a body containing
`Welcome to nginx!`. Confirmed with `--mem 512`, guest port `8080` forwarded
to host port `18082`, and cmdline
`elfloader -- /bin/sh /start.sh`. x86_64 has never been run;
`.github/workflows/e2e-unikraft-examples.yml` runs it as `strict: false`
until its first green run.

Boot prints one non-fatal error before the network comes up:

```
ERR:  [libvirtio_bus] <virtio_bus.c @  141>  Failed to find the driver for the virtio device 0x40001f020 (id:5)
ERR:  [libvirtio_mmio] <virtio_mmio.c @  544>  Failed to register the virtio device: -14
ERR:  [libukbus_platform] <platform_bus.c @  118>  Platform Failed to initialize device driver, ret(-14)
```

This is libkrun attaching a device (id 5, ahead of the NIC) that Unikraft has
no driver for -- almost certainly the memory balloon, the same one noted for
x86_64 in `../../library/unikraft-base/README.md` (patch 5). On arm64 the
guest discovers devices from the device tree rather than the cmdline probe
that patch fixes, so the failed device is simply skipped and everything after
it -- entropy, network -- still registers; `en1` comes up a few lines later
and the server answers normally. Cosmetic, not investigated further.

Of the servers in this examples directory, nginx needed the same
argv-trampoline treatment as `../unikraft-redis`, for the same underlying
reason (see below), but nothing else: it is one process, one worker, no
forking, no filesystem writes beyond its own log files.

## Differences from upstream

**No `runtime: base-compat:latest`.** Upstream pulls a prebuilt Unikraft
Cloud kernel, which is published for x86_64 only. The Kraftfile here builds
the equivalent runtime (`library/base` from `unikraft/catalog`) from source,
plus the arm64 fixes in `../../library/unikraft-base`.

**The Dockerfile resolves its libraries instead of listing them.** Upstream
hardcodes six `/lib/x86_64-linux-gnu/...` paths and
`/lib64/ld-linux-x86-64.so.2`; on arm64 those paths do not exist (there it's
`/lib/aarch64-linux-gnu/...` and `/lib/ld-linux-aarch64.so.1`). `ldd` over
the nginx binary keeps the list correct on both architectures. nginx.conf
here loads no dynamic modules (no `load_module` directive), so
`/usr/lib/nginx/modules` -- five `.so` files in the upstream image -- ships
in neither.

**No `/etc/ld.so.cache`.** Upstream copies the host's cache verbatim, which
is glibc's own resolved-path index for the *build* image's library layout;
nothing here regenerates or needs it; nginx starts and resolves its shared
libraries fine without one (glibc falls back to the compiled-in search path
plus `LD_LIBRARY_PATH`, which `CONFIG_LIBPOSIX_ENVIRON_ENVP1` in the
Kraftfile sets).

**No `/etc/hosts` or `/etc/passwd`-equivalent from upstream.** Upstream ships
its build image's real `/etc/passwd` and `/etc/group` (with an `nginx` user)
because its `nginx.conf` runs workers as `user nginx;`. This port's
`nginx.conf` uses `user root root;` instead (see below), so `/etc/passwd` and
`/etc/group` here are two-line files carrying only the `root` entry each --
enough for the `user` directive's `getpwnam()`/`getgrnam()` lookups at
startup, nothing else. `/etc/resolv.conf`, `/etc/hosts` and `/etc/hostname`
are dropped entirely: app-elfloader autogenerates them at boot
(`CONFIG_APPELFLOADER_AUTOGEN_*` in the Kraftfile).

**`user root root;`, not `user nginx;`.** There is no unprivileged `nginx`
user to drop to, and nothing to drop from -- there is exactly one process
here, no forked workers to isolate. `master_process off;` (from upstream,
kept) makes the `user` directive close to a no-op at runtime regardless; it
is set only so config parsing does not fail looking for a group/user that
does not exist.

**The boot command is `sh /start.sh`, not nginx directly.** libkrun appends
its own words (`earlycon=...`, `tsi_hijack`, a bare `--`) to the end of the
kernel command line, past the `--` stop sequence, so they arrive in the
application's argv; see `../../library/unikraft-base/README.md`. Verified
directly against the upstream nginx binary before building anything:

```
$ nginx -c /etc/nginx/nginx.conf earlycon=pl011,mmio32,0x0a001000 tsi_hijack --
nginx: invalid option: "earlycon=pl011,mmio32,0x0a001000"
```

nginx's own argv parser (`ngx_get_options`) rejects the first word that does
not start with `-`, the same failure mode `../unikraft-redis` hit with
redis-server's config-directive parser, just a different error text. The
start script soaks the junk up as positional parameters and `exec`s nginx
with a clean argv -- one execve(), no fork, enabled by
`CONFIG_APPELFLOADER_MULTIPROCESS` exactly as in `../unikraft-redis` and
`../unikraft-postgres`. The shell is the statically linked busybox
(`busybox:stable-musl`, ~1 MiB), same as `../unikraft-redis`.

## Layout

| file                        | role                                                         |
|------------------------------|---------------------------------------------------------------|
| `Dockerfile`                | rootfs: `nginx`, its libraries (via `ldd`), busybox, config    |
| `rootfs/etc/nginx/nginx.conf` | listens on 8080, serves `/wwwroot`, `master_process off`     |
| `rootfs/etc/passwd`, `rootfs/etc/group` | trimmed `root`-only entries for the `user` directive |
| `rootfs/wwwroot/index.html` | upstream nginx's default "Welcome to nginx!" page              |
| `start.sh`                  | argv filter: soaks up libkrun's junk, `exec`s nginx             |
| `Kraftfile`                 | the from-source base runtime + elfloader + MULTIPROCESS for exec |
| `build.sh`                  | two-phase build; see `../unikraft-postgres/build.sh`            |
