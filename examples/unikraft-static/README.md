# Static site on Unikraft

A directory of HTML and CSS, served by Caddy as a Unikraft unikernel, built
with `bsdkrun pack`.

There is no `Dockerfile`, no `Kraftfile`, no `build.sh` — and no application
code either. `pack` recognises the site, compiles a web server for it, and
generates everything else.

## Build

```sh
bsdkrun pack .
```

## Run

```sh
bsdkrun unikraft . --cmdline "static -- /usr/bin/caddy run --config /etc/caddy/Caddyfile"
```

`pack` prints that command when it finishes.

## Try it

```sh
curl http://<vm-ip>:8080/
```

## What counts as a static site

`pack` picks this provider when it finds any of:

| Condition                          | Notes                                          |
| ---------------------------------- | ---------------------------------------------- |
| `BSDKRUN_STATIC_FILE_ROOT` is set  | Names the directory to serve; wins over the rest |
| A `Staticfile` in the root         | Its `root:` key selects the directory          |
| An `index.html` in the root        | The project root is served as-is               |
| A non-empty `dist/`, `build/`, `out/`, `_site/` or `public/` | First match wins, in that order |

An *empty* `public/` does not count. A leftover directory is not a site, and
matching it would let this provider shadow whatever the project really is.

For the same reason, static is the **last** provider consulted. Most web
frameworks ship an `index.html` or build into `public/`; serving those as flat
files instead of running the app would be a silent and total failure, so any
language provider claims the project first.

## Why Caddy, and why from source

Caddy is a single static Go binary — no ELF interpreter, no shared libraries,
nothing to resolve into the rootfs with `ldd`. The whole guest is one file plus
the site.

It is compiled rather than copied out of the official `caddy` image, which would
otherwise be cheaper and faster. A released Go binary for linux/amd64 is a
non-PIE `ET_EXEC` linked at `0x400000`, and the Unikraft `fc` kernel already
occupies that address: a prebuilt Caddy gets mapped over the running kernel and
the guest dies without printing anything. Relinking it elsewhere
(`-T 0x40000000`) is only possible at build time. See
[`../unikraft-go/README.md`](../unikraft-go/README.md) for the loader trace
behind that failure.

`arm64` links at `0x10000` with the kernel 2 GiB away, and needs no such move.

## Single-page apps

The generated Caddyfile ends with:

```
try_files {path} {path}/ /index.html
```

so a client-side router survives a reload of `/about`. For a plain site this
changes nothing — `index.html` is what would have been served anyway.
