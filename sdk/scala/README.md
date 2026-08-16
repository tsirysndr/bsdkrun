# bsdkrun — Scala SDK

A Scala 3 SDK for [bsdkrun](https://github.com/tsirysndr/bsdkrun), a
Firecracker-style microVM launcher for BSD, Linux and unikernel guests.

Locally it builds argv and shells out to the `bsdkrun` binary; remotely,
[`Client`](#connecting-to-a-remote-daemon) drives the same operations against a
`bsdkrund` daemon's GraphQL API.

Calls are **blocking and return `Either[BsdkrunError, A]`**, so they compose in a
`for` comprehension and the compiler keeps track of what can still fail. That
matches every other bsdkrun SDK — all of them are blocking — and means the only
dependency is upickle: HTTP and WebSocket come from `java.net.http`, so the SDK
drops into a codebase already committed to cats-effect, ZIO, or neither.

## Install

```scala
libraryDependencies += "io.github.tsirysndr" %% "bsdkrun" % "0.1.0"
```

Needs Java 11+ at runtime. The SDK finds the `bsdkrun` binary via
`BSDKRUN_BIN`, then `PATH`, then a dev build in a repo checkout —
`Bsdkrun`'s `Binary.setPath(...)` overrides all of it.

### Toolchain

The JDK, Scala and sbt versions are pinned in `mise.toml`, so a checkout builds
the same way everywhere:

```sh
mise install    # in sdk/scala
sbt test
```

## Creating a sandbox

Builders per guest kind, all ending at `create()`:

```scala
import bsdkrun.*

val sbx = Sandbox.linux("alpine")
  .cpus(2)
  .mem(1024)
  .port("8080:80")
  .command("sleep", "300")
  .create()                       // Either[BsdkrunError, Sandbox]
```

`Sandbox.freebsd()`, `.netbsd()`, `.firmware(fw, disk)`, `.kernel(path)`,
`.unikraft(path)`, `.solo5(path)`, `.nanos(image)` and `.osv(image)` cover the
other guest kinds. `toArgs` shows the exact command line without running it:

```scala
Sandbox.linux("alpine").cpus(2).toArgs
// Right(List(linux, alpine, -d, --cpus, 2))
```

### Environment variables

`env` sets the guest environment for the machine's entrypoint. It merges over
the image's own config, so a key the image already defines is replaced rather
than duplicated, and repeated calls accumulate:

```scala
Sandbox.linux("node:22")
  .env("NODE_ENV", "production")
  .envs(Map("PORT" -> "3000"))
  .command("node", "server.js")
  .create()
```

Linux guests only — BSD guests boot their own init, so there is no generated
init to export into; set those from `exec` after boot. Entries are emitted
sorted by key, so the argv does not depend on the order you added them.

## Running commands

```scala
for
  sbx <- Sandbox.linux("alpine").command("sleep", "300").create()
  out <- sbx.exec("uname", "-a")
  _   <- sbx.stop()
yield out.text
```

`exec` takes argv directly, with no shell parsing. `ExecOptions` adds `env`,
`cwd`, `tty`, `stdin` and streaming callbacks. For a shell, `sh` pairs with an
interpolator that quotes every value — an interpolated string is data, never
syntax:

```scala
import bsdkrun.Shell.sh

sbx.sh(sh"echo $userInput")          // safe even if userInput is "; rm -rf /"
sbx.sh(sh"ls ${Shell.raw("-la")} $dir")   // raw opts a fragment out of quoting
```

A non-zero exit is *not* an error — it comes back in the result, because a
command that fails is an answer. `.checked` turns one into a `Left` when you
want the comprehension to short-circuit.

## Files

`sbx.fs` reads and writes files in the guest. Parent directories are created
for you, and everything is byte-exact — `readFile` hands back an `Array[Byte]`,
so a PNG survives the round trip.

```scala
sbx.fs.writeFile("/app/main.py", "print('hi')")
sbx.fs.readText("/app/out.json")
sbx.fs.readFile("/app/logo.png")

sbx.fs.upload("./src", "/app/src")                        // file or directory
sbx.fs.download("/app/dist", "./dist", recursive = true)
```

`upload` looks at the local path to decide whether to recurse; `download`
cannot (the path is in the guest), so pass `recursive` for a directory. A
directory's *contents* land in the destination.

## Caching

`sbx.cache` saves a guest directory under a key and restores it later, so a
rebuild can pick up where the last one left off. **A miss is not an error** —
check `restored`:

```scala
for
  hit <- sbx.cache.restore(key, restoreKeys = Seq("deps-"))
  _   <- if hit.restored then Right(())
         else sbx.exec("npm", "ci").flatMap(_ => sbx.cache.save("/app/node_modules", key))
yield ()

Cache.list()              // every stored entry, newest first
Cache.remove(Seq(key))    // or Cache.removeAll()
```

`restoreKeys` are prefixes tried in order when the exact key misses; within a
prefix the newest matching entry wins, and `hit.key` says which one was used.
Formats are `Compression.Gzip` (default), `Zstd`, `Estargz` and `Uncompressed`.

Where entries live — host disk or S3 — is host configuration, not an SDK
concern: set `BSDKRUN_CACHE_BACKEND` / `BSDKRUN_CACHE_S3_*`, or write
`~/.config/bsdkrun/cache.toml`.

## Lifecycle & inventory

```scala
sbx.stop()            // returns the Sandbox, so lifecycle calls chain
sbx.start()
sbx.update(cpus = Some(4))
sbx.remove(force = true)
sbx.commit("my-flavor")

Sandbox.list(all = true)
Sandbox.get("web")
sbx.status()          // Either[_, Option[SandboxInfo]]
sbx.logs()
sbx.shell()           // inherits this process's terminal
```

## Host operations

```scala
Images.list()
Volumes.list()  / Volumes.remove(Seq("cache"))
Networks.list() / .create("devnet") / .connect("devnet", id)
Host.probe()    / Host.doctor() / Host.fetchImage("freebsd")
```

`Host` rather than `System`, because `java.lang.System` is auto-imported into
every Scala file — an `object System` here would shadow it for anyone doing
`import bsdkrun.*`.

## Connecting to a remote daemon

`Client` drives a remote `bsdkrund` over GraphQL: queries and mutations over
HTTP, subscriptions over a shared WebSocket speaking `graphql-transport-ws`.

```scala
for
  client <- Client.fromEnv()          // BSDKRUN_URL + BSDKRUN_TOKEN
  rows   <- client.listMachines(all = true)
  out    <- client.exec(id, Seq("uname", "-a"))
yield out.text

// streaming
val stop = client.followLogs(id, onLine = println)

// interactive
val session = client.shell(id).map(_.onOutput(bytes => print(new String(bytes))))
```

A URL set without a token is an error rather than a silent unauthenticated
fallback, and an `UNAUTHENTICATED` response becomes `BsdkrunError.Auth`
specifically, so a caller does not retry a bad token forever.

`client.request(query, variables)` is the escape hatch for any document the SDK
has no typed wrapper for.

## Errors

Everything fails through `BsdkrunError`, a sealed hierarchy — `BinaryNotFound`,
`CommandFailed`, `SandboxNotFound`, `FileTransfer`, `InvalidOptions`,
`DecodeFailed`, `GraphQL`, `Auth`, `MissingConfig`. Match on it, or use the
`*OrThrow` variants to get a `BsdkrunException` instead.

## Development

```sh
mise install       # pinned JDK / Scala / sbt
sbt test           # unit suites

# the e2e suite boots a real microVM; it skips unless switched on
BSDKRUN_E2E=1 BSDKRUN_BIN=../../target/release/bsdkrun sbt "testOnly bsdkrun.E2ESuite"
```

## Publishing

The artifact goes to **Maven Central** — the only registry Scala's
`libraryDependencies` resolves from by default. (GitHub Packages works too, but
consumers would need to authenticate to fetch it, which is wrong for a public
SDK.)

The coordinate is `io.github.tsirysndr:bsdkrun_3`. sbt's `%%` appends the
Scala binary suffix, so it does not collide with the Clojure SDK's
`io.github.tsirysndr/bsdkrun` — which lives on Clojars in any case.

One-time setup, none of which this repo can do for you:

1. A [Central Portal](https://central.sonatype.com) account, with the
   `io.github.tsirysndr` namespace verified. An `io.github.<user>` namespace is
   verified by proving you own that GitHub account; a custom domain would need
   a DNS TXT record instead.
2. A GPG key **published to a keyserver**. Central verifies the signature and
   rejects a key it cannot find, so signing with an unpublished key produces a
   bundle that fails only at upload. `build.sbt` pins `pgpSigningKey` to the
   published one rather than letting gpg pick its default — check yours with:

   ```sh
   curl -sI "https://keys.openpgp.org/vks/v1/by-keyid/<KEYID>" | head -1
   ```

   On macOS, signing from sbt also needs a **GUI pinentry**, because sbt gives
   its `gpg` subprocess no terminal and the default curses pinentry then fails
   with `Inappropriate ioctl for device`:

   ```sh
   brew install pinentry-mac
   echo "pinentry-program $(brew --prefix)/bin/pinentry-mac" >> ~/.gnupg/gpg-agent.conf
   gpgconf --kill gpg-agent
   ```

3. A **Central Portal user token** — <https://central.sonatype.com> → your
   account → *Generate User Token*. That yields a token username and password;
   your login credentials will not work. They go in the environment, not in a
   file:

   ```sh
   export CENTRAL_TOKEN_USERNAME=...
   export CENTRAL_TOKEN_PASSWORD=...
   ```

Then, from `sdk/scala`:

```sh
sbt publishSigned      # stage a signed bundle under target/sonatype-staging
./publish-central.sh   # zip it and POST it to the Portal
```

Or through the monorepo console, which runs both:

```sh
bb sdk:publish scala   # from tools/console
```

`publish-central.sh` leaves the deployment staged for you to review and release
at <https://central.sonatype.com/publishing/deployments>; pass
`PUBLISHING_TYPE=AUTOMATIC` to release as soon as validation passes.

### Why not `sbt sonatypeBundleRelease`

sbt-sonatype (3.12.2, the newest) speaks the **legacy Nexus staging API**, which
the Central Portal does not serve — `https://central.sonatype.com/service/local/…`
returns a 404 HTML page. Sonatype runs `ossrh-staging-api.central.sonatype.com`
as a compatibility host, and the plugin authenticates against it, but it
implements only part of that API and the release path dies on:

```
400 Bad Request: Endpoint /service/local/staging/profile_repositories not supported
```

So the plugin is used only for what it does well — building a correctly signed
bundle in Maven layout — and the upload goes straight to the Portal's own API.
That also means **no `~/.sbt/1.0/sonatype.sbt` is needed**; nothing in the
staging step authenticates.

`sbt publishM2` installs to `~/.m2` for testing the artifact locally without
touching the registry. Central requires the `-sources` and `-javadoc` jars and
a POM carrying name/description/url/licenses/scm/developers; `build.sbt` sets
all of that, so a plain `sbt package` is *not* a publishable build — use
`bb sdk:build scala`, which runs `package packageSrc packageDoc`.

## License

MIT
