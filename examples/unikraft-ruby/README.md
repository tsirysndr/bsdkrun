# unikraft-ruby

Ruby 3.2 serving HTTP from a `TCPServer` loop, running as a Unikraft
unikernel. Ported from [`unikraft-cloud/examples`'s
`httpserver-ruby3.2`](https://github.com/unikraft-cloud/examples/tree/main/httpserver-ruby3.2)
to build for **arm64** as well as x86_64 and boot under bsdkrun.

```sh
bsdkrun pack .                # host arch; or: bsdkrun pack . --target x86_64
bsdkrun unikraft . --mem 512 --port 8080:8080 \
  --cmdline "elfloader -- /usr/bin/ruby /src/server.rb"
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

`server.rb` is upstream's, verbatim: one thread, one connection at a time.
Extra words in `ARGV` (libkrun appends some — see
`../unikraft-redis/README.md`) are ignored by the script, so no trampoline is
needed.

## Differences from upstream

**No `runtime: base-compat:latest`.** Upstream pulls a prebuilt Unikraft Cloud
kernel, which is published for x86_64 only. The Kraftfile here builds the
equivalent runtime (`library/base` from `unikraft/catalog`) from source, plus
the arm64 fixes in `../../library/unikraft-base`.

**The Dockerfile resolves its libraries instead of listing them.** Upstream
enumerates `/lib/x86_64-linux-gnu/...` paths (and pins
`--platform=linux/x86_64` to make that true); on arm64 those paths do not
exist. `ldd` — over the ruby binary *and* every stdlib `.so`, since native
extensions like `socket.so` are dlopen()ed and their libraries (libz, libssl,
libyaml, ...) would otherwise be missing — keeps the list correct on both
architectures. `libruby.so` lands in place the same way, because `ldd`
reports it with its full path.

## Layout

| file         | role                                                      |
|--------------|-----------------------------------------------------------|
| `server.rb`  | upstream's TCPServer HTTP loop, verbatim                  |
