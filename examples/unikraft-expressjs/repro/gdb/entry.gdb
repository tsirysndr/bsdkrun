set pagination off
set confirm off
set architecture aarch64
set can-use-hw-watchpoints 1
target remote host.docker.internal:1234
set $ldb = 0x10003c0000
hbreak *($ldb + 0x6b584)
continue
printf "\n=== load_library ENTRY\n"
printf "  x30 (return addr) = %#lx", $x30
if ($x30 >= $ldb)
  printf "  = ld+%#lx", $x30 - $ldb
end
printf "\n  x29 = %#lx\n  sp  = %#lx\n  x0  = %#lx  \"%s\"\n", $x29, $sp, $x0, (char*)$x0
