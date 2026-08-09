set pagination off
set confirm off
set architecture aarch64
target remote host.docker.internal:1234
set $ldb = 0x10003c0000

hbreak *($ldb + 0x6b590)
continue
set $slot = $sp + 8
delete
printf "\n== load_library entry: slot %#lx = %#lx (ld+%#lx)\n", $slot, *(unsigned long*)$slot, *(unsigned long*)$slot - $ldb

hbreak *($ldb + 0x20b98)
hbreak *($ldb + 0x3df4c)
hbreak *($ldb + 0x685ac)
set $i = 0
while ($i < 10)
  continue
  set $v = *(unsigned long*)$slot
  printf "[%2d] stopped at ld+%#lx   slot=%#lx\n", $i, $pc - $ldb, $v
  set $i = $i + 1
end
