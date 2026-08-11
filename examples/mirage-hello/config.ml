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

(* mirage-crypto 2.2.0 added an entropy self-test that `initialize` runs at
   startup, unconditionally, in every unikernel. It calls the cycle counter
   eleven times and fails if any two consecutive reads are equal:

     Fatal error: exception Failure("same data from timer at 3 with: ...")
     Solo5: solo5_exit(2) called

   On aarch64 that counter is CNTVCT_EL0, and on Apple silicon consecutive
   reads *are* frequently equal — its update granularity is coarser than its
   nominal 1 GHz. Measured on the host, outside any VM, 7 of 11 back-to-back
   reads returned the same value, so this is a property of the CPU rather than
   of the tender or of Hypervisor.framework. The unikernel then dies before
   main(), which reads as a bsdkrun bug and is not one.

   x86_64 is unaffected: there the counter is RDTSC, which ticks fast enough
   that no two reads collide. Constraining this here rather than only on the
   platforms that need it keeps CI building what macOS runs. *)
let crypto_without_the_entropy_self_test =
  [
    package ~max:"2.2.0" "mirage-crypto";
    package ~max:"2.2.0" "mirage-crypto-rng-mirage";
  ]

let main =
  main "Unikernel.Main"
    ~packages:crypto_without_the_entropy_self_test
    (stackv4v6 @-> job)

let () = register "hello" [ main $ stack ]
