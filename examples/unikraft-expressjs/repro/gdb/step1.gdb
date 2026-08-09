set pagination off
set confirm off
set architecture aarch64
target remote host.docker.internal:1234

# ld-musl load bias, from the loader's own trace
set $ldb = 0x10003c0000

# close() inside musl's load_library, right after map_library() returns.
hbreak *($ldb + 0x685ac)
continue
printf "\n=== hit close(): pc=%#lx  lr=%#lx (ld+%#lx)  x0=%#lx\n", $pc, $lr, $lr - $ldb, $x0
