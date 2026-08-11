# mirage-hello

A [MirageOS](https://mirage.io) unikernel serving one HTTP response over a real
TCP/IP stack, run by the **Solo5 `hvt` tender** that `bsdkrun` embeds.

```sh
./build.sh                                              # needs opam + mirage
bsdkrun solo5 dist/hello.hvt --mem 128 --port 18080:8080
curl http://127.0.0.1:18080/                            # Hello from MirageOS on bsdkrun
```

`bsdkrun mirage` is an alias for `bsdkrun solo5`, so the second line can also
be written `bsdkrun mirage dist/hello.hvt …`.

Notice what is *not* on that command line: no device names, no MAC, no IP, no
gateway. The unikernel declares the devices it wants inside its own binary and
leases its address over DHCP — see below.

## What runs it

Unlike every other guest in bsdkrun, a Solo5 unikernel does not run through
libkrun. It is run by `solo5-hvt`, a tender that drives Hypervisor.framework
(macOS) or KVM (Linux) itself, in its own process. bsdkrun builds that tender
from the pinned `library/solo5` submodule at compile time and embeds it, so
**running** a unikernel needs no Solo5 install of your own.

**Building** one still needs the MirageOS toolchain — a cross-compiler that
emits ELF for a bare-metal target — which is what `build.sh` drives:

```sh
opam install mirage
./build.sh          # mirage configure -t hvt && make depends && make build
```

`-t hvt` matters. The other Solo5 targets (`spt`, `virtio`, `xen`, …) produce
binaries this tender cannot load; `bsdkrun solo5` reads the ABI note and names
the mismatch rather than letting the tender fail with an ELF error.

## Devices come from the binary, not the command line

Every Solo5 unikernel carries a manifest (`MFT1` ELF note) listing the devices
it declares, by name. The tender refuses to boot unless every one of them is
attached — so bsdkrun reads the manifest and attaches them itself:

```
INFO running Solo5 unikernel id=4bc3fd4a9851 image=hello.hvt nets=["service"] blocks=[]
```

`service` is the name `config.ml` gives the network. It reaches the outside
through gvproxy, which bsdkrun connects to and hands the tender as a file
descriptor (`--net:service=@3`) — the only way to attach a network on macOS,
which has no TAP device.

A unikernel that declares a block device is told which file backs it:

```sh
bsdkrun solo5 dist/store.hvt --block storage=disk.img
```

Leave it out and bsdkrun says which device is missing before starting anything,
rather than letting the tender fail after the network is already up.

## The address is leased, not configured

`config.ml` pins DHCP at configure time:

```ocaml
let stack = generic_stackv4v6 ~dhcp_key:(Key.pure true) default_network
```

Without that, `generic_stackv4v6` reads a `--dhcp` runtime flag that **defaults
to false**, the unikernel comes up on mirage's built-in 10.0.0.2, answers
nothing, and every boot has to carry `--ipv4=…`/`--ipv4-gateway=…` matching
whatever address bsdkrun's network happens to use. With it, the unikernel
leases from gvproxy's DHCP server exactly as the FreeBSD, NetBSD and Linux
guests do:

```
[dhcp_client_lwt] Lease obtained! IP: 192.168.127.2, routers: 192.168.127.1
[application] listening on port 8080
```

The lease lands on `192.168.127.2` — the address `--port` forwards to —
because bsdkrun passes the tender a fixed MAC (`--net-mac:service=…`). Left to
generate its own, the tender picks a random one per boot, gvproxy leases it
`.3`, `.4`, … and the port forward points at nobody. That is the one piece of
this that is not automatic in Solo5 itself.

## Running it like any other machine

The unikernel is recorded in the same database as every other guest, so it
detaches and is managed the usual way:

```sh
id=$(bsdkrun solo5 dist/hello.hvt --mem 128 --port 18080:8080 -d)
bsdkrun ps                  # Up 12 seconds   127.0.0.1:18080->8080/tcp
bsdkrun logs "$id"
bsdkrun stop "$id"
```

`exec` and `shell` do not apply — a unikernel has no shell and no agent to run
one. `stop` kills the tender rather than just bsdkrun, so a stopped machine
leaves nothing behind holding its ports.

## Status

Verified on **macOS/arm64 (Hypervisor.framework)**: builds, boots, leases a
DHCP address and serves the body above. Linux/x86_64 (KVM) is covered by the
`e2e-solo5` workflow, which asserts on the served response rather than on the
job's conclusion.

The tender comes from
[tsirysndr/solo5](https://github.com/tsirysndr/solo5) `hvf-macos-aarch64`,
which adds the macOS/HVF backend upstream Solo5 does not have. Two gaps it
documents rather than papers over: the tender drops no privileges on macOS
(there is no seccomp/pledge/capsicum equivalent), and there is no
`solo5-hvt-debug`, the gdb and dumpcore backends being unported.
