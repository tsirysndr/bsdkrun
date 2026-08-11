open Mirage

(* One network device, which the unikernel declares in its own binary as the
   Solo5 manifest entry `service`. That name is not decoration: the tender
   refuses to boot unless every declared device is attached, and `bsdkrun solo5`
   reads the manifest out of the ELF to know what to attach it to.

   The stack leases its address over DHCP, which is what keeps the boot command
   free of network arguments — gvproxy runs a DHCP server, exactly as it does
   for the FreeBSD, NetBSD and Linux guests. That is the *default* here only
   because this needs mirage >= 4.11: it inverted the old `dhcp` key into
   `no_dhcp` and flipped the default, so DHCP is what you get unless you pass
   `--no-dhcp`.

   On mirage 4.10 and earlier the same line compiles and silently does the
   opposite — a static 10.0.0.2 that answers nothing, because `dhcp` defaulted
   to false and had to be pinned with `~dhcp_key:(Key.pure true)`, a labelled
   argument 4.11 removed. build.sh checks the version rather than leaving that
   to be discovered as an unreachable server. *)
let stack = generic_stackv4v6 default_network

let main = main "Unikernel.Main" (stackv4v6 @-> job)

let () = register "hello" [ main $ stack ]
