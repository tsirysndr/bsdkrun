# Haskell on Unikraft

A Haskell HTTP service running as a Unikraft unikernel, built with `bsdkrun pack`.

Detected by a `package.yaml` **and** Haskell source. `package.yaml` is an
ordinary name for an ordinary config file, so on its own it would claim projects
with no Haskell in them.

## Build

```sh
bsdkrun pack .
```

## Run

```sh
bsdkrun unikraft . --cmdline "haskell -- /usr/bin/server"
```

## Try it

```sh
curl http://<vm-ip>:8080/
```

## Two version traps

**The image tag.** `haskell:9.4` is Debian buster, whose repositories are
archived — any `apt-get update` against it fails outright and takes the build
with it. This uses `haskell:9.6`, which is bullseye. Nothing is installed with
apt anyway: the image already carries the certificates Stack needs for Hackage.

**The compiler.** A Stackage snapshot names an exact GHC, and the image ships
whatever its tag last built with: `lts-22.28` wants 9.6.6 and `haskell:9.6`
carries 9.6.7. Stack treats that as a hard *"No compiler found"* rather than a
warning, so `stack.yaml` sets `compiler-check: newer-minor`. `pack` writes the
same setting into any `stack.yaml` it generates for a project that has none.

Stack is also told `--system-ghc --no-install-ghc`. Without them it ignores the
compiler already in the image and downloads several hundred megabytes to arrive
at the same one.

## Raw sockets

The server uses the `network` package directly rather than warp: this example is
about the toolchain reaching the guest, and a web server would bring a hundred
transitive dependencies with it.

GHC links against libgmp, libffi and libc dynamically, so unlike the Go, Zig and
Crystal examples the binary is not self-contained — those libraries are resolved
into the rootfs with `ldd`.
