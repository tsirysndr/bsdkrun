set pagination off
set confirm off
set architecture aarch64
target remote host.docker.internal:1234
set $ldb = 0x10003c0000
set $slot = 0x1000382618

hbreak *($ldb + 0x527f0)
continue
delete
printf "\n== at fstat entry: fd=%d buf=%#lx slot=%#lx\n", $x0, $x1, *(unsigned long*)$slot

set $prev = *(unsigned long*)$slot
set $i = 0
while ($i < 60000)
  stepi
  set $v = *(unsigned long*)$slot
  if ($v != $prev)
    printf "\n== slot changed after %d steps: %#lx -> %#lx\n", $i, $prev, $v
    printf "   pc = %#lx  sp = %#lx\n", $pc, $sp
    set $i = 99999
  end
  set $i = $i + 1
end
printf "\n=== instructions around pc:\n"
x/6i $pc - 20
info registers x0 x1 x2 x3 x4 sp
