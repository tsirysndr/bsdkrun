# Crystal on Unikraft

A Crystal HTTP service, compiled to a **static binary** — no ELF interpreter and
no shared libraries, so the guest is one file plus the kernel.

The provider builds on Alpine because `--static` needs musl: glibc does not
support fully static linking for anything using NSS, and Crystal's socket code
does. On x86_64 it also relinks the binary to `0x40000000`, clear of the
Unikraft `fc` kernel that occupies the `0x400000` a static ET_EXEC would
otherwise load at.

Detected by `shard.yml`; the binary and its entry point come from that file's
`targets:` block.

There is no `Dockerfile`, no `Kraftfile` and no `build.sh` — `bsdkrun pack`
detects the project and generates all three internally.

## Build

```sh
bsdkrun pack .
```

## Run

```sh
bsdkrun unikraft . --cmdline "crystal -- /usr/bin/server"
```

`pack` prints that command when it finishes.

## Try it

```sh
curl http://<vm-ip>:8080/
```

## Publish it

```sh
bsdkrun pack . --push ghcr.io/you/crystal:v1
bsdkrun unikraft ghcr.io/you/crystal:v1
```

The second command needs no copy of this directory: the kernel is pulled on
first use and cached, and the argv comes from the image.
