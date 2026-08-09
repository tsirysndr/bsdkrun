# Confirm what the spin is waiting for, by supplying it by hand.
#
# The guest is stopped inside IB_thread::start(), busy-waiting on the thread
# state word for the thread it has just created to publish STARTED. x0 holds
# the IB_thread, and the state is a std::atomic<thread_state_t> whose address
# lives at [x0 + 16]:
#
#   State { INVALID=0, NOT_STARTED=1, ALLOWED_TO_START=2, STARTED=3,
#           STOPPING=4, STOPPED=5 }
#
# start() asserts NOT_STARTED (cmp #1), stores ALLOWED_TO_START (#2), and then
# loops while the word still reads 2. Writing 3 is exactly what the child would
# have done had it ever been scheduled.
#
# If the guest then makes progress, the spin was waiting on nothing but that
# store -- i.e. the child never ran -- rather than on any work the child does.
#
# Expect the child to hit an assertion shortly afterwards:
#
#   [InnoDB] Assertion failure: os0thread-create.h:185:
#     m_thread.state() == IB_thread::State::ALLOWED_TO_START
#
# That is this script's doing, not a second bug: the child checks the state it
# was left in, and this forged it. Its firing is the proof that the child was
# runnable and merely starved.
#
#   gdb-multiarch -q -batch -x poke-started.gdb .unikraft/build/mysql_qemu-arm64.dbg

set pagination off
set confirm off
set height 0

target remote host.docker.internal:1234

printf "PC = %#lx  (expected: inside IB_thread::start)\n", $pc

set $obj   = $x0
set $state = *(unsigned long *)($obj + 16)
printf "IB_thread   = %#lx\n", $obj
printf "state addr  = %#lx\n", $state
printf "state value = %u   (2 = ALLOWED_TO_START)\n", *(unsigned int *)$state

printf "\n-- writing STARTED (3), the store the child never made --\n"
set *(unsigned int *)$state = 3
printf "state value = %u\n", *(unsigned int *)$state

# Detaching resumes the guest. Watch the console: if the boot moves on, the
# parent was blocked solely on that word.
detach
