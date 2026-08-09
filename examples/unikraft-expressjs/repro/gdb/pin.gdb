set pagination off
set confirm off
set architecture aarch64
target remote host.docker.internal:1234
set $ldb = 0x10003c0000

hbreak *($ldb + 0x6b590)
continue
set $slot = $sp + 8
delete
printf "\n== load_library entry: slot=%#lx\n", *(unsigned long*)$slot

hbreak *($ldb + 0x20a28)
hbreak *($ldb + 0x527f0)
hbreak *($ldb + 0x6903c)
hbreak *($ldb + 0x3df4c)
set $i = 0
while ($i < 8)
  continue
  printf "[%d] entry of ", $i
  if ($pc == $ldb + 0x20a28)
    printf "fcntl"
  end
  if ($pc == $ldb + 0x527f0)
    printf "fstat  (buf=%#lx, buf+128=%#lx)", $x1, $x1 + 128
  end
  if ($pc == $ldb + 0x6903c)
    printf "read"
  end
  if ($pc == $ldb + 0x3df4c)
    printf "mmap"
  end
  printf "  ->  slot=%#lx\n", *(unsigned long*)$slot
  set $i = $i + 1
end
