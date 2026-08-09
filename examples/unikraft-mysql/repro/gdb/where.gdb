# Attach to a wedged guest and find out what it is executing.
#
# Connecting to QEMU's stub halts the vCPU wherever it happens to be, so this
# needs no breakpoints: it prints the program counter, disassembles around it,
# and single-steps a little to see whether the guest is in a tight loop.
#
# Unikraft links its kernel low (0x40000000-ish on the qemu/arm64 virt board)
# and maps the application through ukvmem at CONFIG_LIBUKVMEM_DEFAULT_BASE,
# 0x100000000. So a PC above 4 GiB is mysqld and one below it is Unikraft --
# which is the first thing worth knowing. (mysqld turns out to be neither: it
# is a non-PIE executable, so it sits at its link address, ~0x1500000, and its
# own symbols resolve there directly.)
#
# Keep this script short. Every `stepi` is a round trip to the stub, so a big
# step count takes minutes rather than seconds.
#
#   gdb-multiarch -q -batch -x where.gdb .unikraft/build/mysql_qemu-arm64.dbg

set pagination off
set confirm off
set height 0

target remote host.docker.internal:1234

printf "\n=== where the guest stopped on attach ===\n"
printf "PC = %#lx\n", $pc
printf "SP = %#lx\n", $sp
printf "LR = %#lx\n", $x30
info symbol $pc
printf "--- around PC ---\n"
x/8i $pc-16
printf "--- backtrace (kernel code only; meaningless in the application) ---\n"
bt 8

printf "\n=== 16 single-steps: tight loop? ===\n"
set $i = 0
while $i < 16
  printf "  pc=%#lx  ", $pc
  info symbol $pc
  stepi
  set $i = $i + 1
end

printf "\n=== registers ===\n"
info registers pc sp x0 x1 x2 x19 x20 x29 x30

printf "\n=== EL1 state ===\n"
printf "ESR_EL1  = %#lx\n", $ESR_EL1
printf "ELR_EL1  = %#lx\n", $ELR_EL1
printf "FAR_EL1  = %#lx\n", $FAR_EL1
printf "SPSR_EL1 = %#lx\n", $SPSR_EL1

detach
