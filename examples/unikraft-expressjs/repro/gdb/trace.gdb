set pagination off
set confirm off
set architecture aarch64
target remote host.docker.internal:1234
set $ldb = 0x10003c0000

# 1. close() inside musl load_library, immediately after map_library() returned
hbreak *($ldb + 0x685ac)
continue
set $ret = $lr
printf "\n=== close() called from ld+%#lx\n", $ret - $ldb
delete

# 2. return into load_library
hbreak *$ret
continue
printf "=== back in load_library at ld+%#lx\n", $pc - $ldb
delete

# 3. from here on, log every translation block QEMU executes
monitor log exec,nochain
printf "=== tracing enabled, running to the fault\n"
continue
