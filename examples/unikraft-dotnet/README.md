# C# on Unikraft

A C# HTTP service on a **self-contained** .NET runtime.

Detected by any `*.csproj`. The SDK version comes from `CSHARP_SDK_VERSION` or
a `global.json`, reduced to the image's `major.minor` tag — `global.json` pins
a feature band (`8.0.404`), which is not a tag that exists.

`--self-contained`, so the runtime ships with the app: there is no package
manager in the guest to install one, and a framework-dependent publish would look
fine at build time and fail to start in the guest.

Trimming is deliberately **off**. It decides what to keep by static analysis, and
anything reached by reflection — which is most of what a framework does at
startup — is invisible to it.

`InvariantGlobalization` is set in the project file: it drops the ICU
dependency, which would otherwise have to be resolved into the rootfs before the
runtime would start at all.

There is no `Dockerfile`, no `Kraftfile` and no `build.sh` — `bsdkrun pack`
detects the project and generates all three internally.

## Build

```sh
bsdkrun pack .
```

## Run

```sh
bsdkrun unikraft . --cmdline "dotnet -- /usr/src/app/server"
```

`pack` prints that command when it finishes.

## Try it

```sh
curl http://<vm-ip>:8080/
```

## Publish it

```sh
bsdkrun pack . --push ghcr.io/you/dotnet:v1
bsdkrun unikraft ghcr.io/you/dotnet:v1
```

The second command needs no copy of this directory: the kernel is pulled on
first use and cached, and the argv comes from the image.
