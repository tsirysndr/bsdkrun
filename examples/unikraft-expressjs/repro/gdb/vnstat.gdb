set pagination off
set confirm off
set architecture aarch64
target remote host.docker.internal:1234
hbreak *0x4014c1e0
continue
printf "\n== vn_stat(vp=%#lx, st=%#lx)\n", $x0, $x1
printf "   st + 128 = %#lx   st + 144 = %#lx\n", $x1 + 128, $x1 + 144
printf "   load_library saved x29/x30 live at 0x1000382610 / 0x1000382618\n"
printf "   before memset: [0x1000382610]=%#lx  [0x1000382618]=%#lx\n", \
       *(unsigned long*)0x1000382610, *(unsigned long*)0x1000382618
