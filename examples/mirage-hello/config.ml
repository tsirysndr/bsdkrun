open Mirage

(* One network device, which the unikernel declares in its own binary as the
   Solo5 manifest entry `service`. That name is not decoration: the tender
   refuses to boot unless every declared device is attached, and `bsdkrun solo5`
   reads the manifest out of the ELF to know what to attach it to.

   `~dhcp_key:(Key.pure true)` is what keeps the boot command free of network
   arguments. `generic_stackv4v6` decides between DHCP and a static address
   from the `--dhcp` runtime flag, which defaults to *false* — so without this
   the unikernel comes up on mirage's default 10.0.0.2 and answers nothing,
   and every boot has to carry `--ipv4=...`/`--ipv4-gateway=...` matching
   whatever bsdkrun's network happens to be. Pinning it at configure time
   instead means the unikernel leases its address from gvproxy's DHCP server,
   exactly as the FreeBSD, NetBSD and Linux guests do. *)
let stack = generic_stackv4v6 ~dhcp_key:(Key.pure true) default_network

let main = main "Unikernel.Main" (stackv4v6 @-> job)

let () = register "hello" [ main $ stack ]
