# unikraft-rails

[Ruby on Rails](https://rubyonrails.org/) 7.1 on Ruby 3.2, served by Puma,
running as a Unikraft unikernel. Ported from [`unikraft-cloud/examples`'s
`ruby3.2-rails`](https://github.com/unikraft-cloud/examples/tree/main/ruby3.2-rails)
to build for **arm64** as well as x86_64 and boot under bsdkrun.

```sh
./build.sh                    # host arch; or: ./build.sh x86_64
bsdkrun unikraft . --mem 2048 --port 3000:3000 \
  --cmdline "elfloader -- /usr/bin/ruby /app/boot.rb"
```

```console
$ curl http://127.0.0.1:3000/
<h1>Hello World</h1>
Hello, World!
```

The application is generated at image build time (`rails new`), then overlaid
with the four files in `app/` and `config/` here: a hello controller, its
view, the routes, and a development-environment tweak.

## Status

Untested as of this writing. x86_64 runs in
`.github/workflows/e2e-unikraft-examples.yml` as `strict: false` until it has
its first green run; arm64 is exercised by hand on
macOS/Hypervisor.framework.

The server runs in development mode, single Puma process, threads only — no
workers, so nothing forks. Everything Rails wants to write at runtime
(`tmp/`, `log/`, the development secret) lands in the ramfs the rootfs is
unpacked into, which is writable and does not survive a reboot.

## Differences from upstream

**The entrypoint is `/app/boot.rb`, not `bin/rails server`.** libkrun appends
its own words (`earlycon=...`, `tsi_hijack`, a bare `--`) to the end of the
kernel command line, past the `--` stop sequence, so they arrive in `ARGV` —
and Thor, the CLI layer under `rails server`, aborts on words it does not
recognise. `boot.rb` clears `ARGV` first, then does exactly what
`bin/rails server -b 0.0.0.0 -p 3000` would have done, handing the server its
options explicitly. (The C-server examples solve the same problem with a
busybox `exec` trampoline — see `../unikraft-redis`; here the interpreter is
already in charge, so no extra process is needed.)

**Rails is pinned to 7.1** (`gem install rails -v '~> 7.1.0'`). Upstream
installs whatever is newest, which moved past Ruby 3.2's reach with Rails
7.2+; the pin keeps the build reproducible on the image upstream chose. 7.1
apps default to importmap, so no node enters the build.

**A root route is added.** Upstream serves only `/hello`; the e2e check does
a plain `GET /`, so `config/routes.rb` also points root at the hello
controller rather than the Rails welcome page.

**No `runtime: base-compat:latest`**, and **libraries resolved with `ldd`**
rather than listed — over the ruby binary and every `.so` in the stdlib *and*
in `/usr/local/bundle` (sqlite3, nokogiri, ...), since native gem extensions
are dlopen()ed. `/usr/share/zoneinfo` ships because ActiveSupport's tzinfo
reads the system zoneinfo on Linux rather than bundling its own.

## Layout

| file                  | role                                                       |
|-----------------------|------------------------------------------------------------|
| `Dockerfile`          | `rails new` + overlay, rootfs via `ldd`                    |
| `boot.rb`             | clears polluted ARGV, invokes the server command           |
| `app/`, `config/`     | controller, view, routes, dev-environment tweak            |
| `Kraftfile`           | the from-source base runtime + preemption/TID + elfloader  |
| `build.sh`            | two-phase build; see `../unikraft-postgres/build.sh`       |
