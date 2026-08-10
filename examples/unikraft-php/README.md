# unikraft-php

PHP 8.2 (CLI) serving HTTP from a raw socket loop, running as a Unikraft
unikernel. Ported from [`unikraft-cloud/examples`'s
`httpserver-php8.2`](https://github.com/unikraft-cloud/examples/tree/main/httpserver-php8.2)
to build for **arm64** as well as x86_64 and boot under bsdkrun.

```sh
./build.sh                    # host arch; or: ./build.sh x86_64
bsdkrun unikraft . --mem 512 --port 8080:8080 \
  --cmdline "elfloader -- /usr/local/bin/php /usr/src/server.php"
```

```console
$ curl http://127.0.0.1:8080/
Hello, World!
```

## Status

**arm64 works.** The unikernel boots, DHCPs an address, the server starts and
answers over the forwarded port with `Hello, World!`. x86_64 has never been
run; `.github/workflows/e2e-unikraft-examples.yml` runs it as `strict: false`
until its first green run.

`server.php` is upstream's, verbatim: a single process accepting one
connection at a time over the `sockets` extension. Extra words in `$argv`
(libkrun appends some — see `../unikraft-redis/README.md`) are ignored by the
script, so no trampoline is needed.

## Differences from upstream

**No `runtime: base-compat:latest`.** Upstream pulls a prebuilt Unikraft Cloud
kernel, which is published for x86_64 only. The Kraftfile here builds the
equivalent runtime (`library/base` from `unikraft/catalog`) from source, plus
the arm64 fixes in `../../library/unikraft-base`.

**The Dockerfile resolves its libraries instead of listing them.** Upstream
enumerates 45 `/lib/x86_64-linux-gnu/...` paths (and pins
`--platform=linux/x86_64` to make that true); on arm64 those paths do not
exist. `ldd` — over the php binary *and* every extension `.so`, since
extensions are dlopen()ed and would otherwise have unresolved libraries —
keeps the list correct on both architectures.

**Only the extension directory ships from `/usr/local/lib/php`.** The rest of
that tree is PEAR, which nothing here uses; every megabyte is paid for twice
at boot (embedded in the kernel image, unpacked into ramfs).

## Layout

| file         | role                                                        |
|--------------|-------------------------------------------------------------|
| `Dockerfile` | rootfs: `php`, extensions, libraries (via `ldd`)            |
| `php.ini`    | loads the sockets extension; compiled-in defaults otherwise |
| `server.php` | upstream's socket-loop HTTP server, verbatim                |
| `Kraftfile`  | the from-source base runtime + elfloader                    |
| `build.sh`   | two-phase build; see `../unikraft-postgres/build.sh`        |
