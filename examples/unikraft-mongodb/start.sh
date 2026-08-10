#!/bin/sh
# Not an init script -- an argv filter. libkrun appends its own words to the
# command line and they land in the boot argv (see the Dockerfile); run as
# `sh /start.sh <junk...>` those words are only this script's positional
# parameters, and mongod gets the argv it expects. `exec` replaces the shell
# in place: an execve(), no fork.
#
# --nounixsocket: nothing in the guest could connect over it, and mongod
# aborts if it cannot create the socket where it expects to.
#
# The cache bound keeps WiredTiger from sizing itself off the guest's total
# memory, which is mostly spoken for by the twice-resident rootfs.
#
# log=(prealloc=false): WiredTiger's log server otherwise keeps a pool of
# pre-created journal files, which it makes as WiredTigerPreplog.NNN and
# renames into place. Under Unikraft's ramfs that cycle fails with ENOENT and
# the server panics a few seconds after startup --
#
#   __log_server:924:log server error ... No such file or directory
#   WT_PANIC: WiredTiger library panic
#
# -- taking mongod with it. Without pre-allocation each journal file is
# created where it is needed. Slower under write load; this is a demo.
exec /usr/bin/mongod --bind_ip_all --nounixsocket \
    --wiredTigerCacheSizeGB 0.25 \
    --wiredTigerEngineConfigString "log=(prealloc=false)"
