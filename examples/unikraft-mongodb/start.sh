#!/bin/sh
# Not an init script -- an argv filter. libkrun appends its own words to the
# command line and they land in the boot argv (see the Dockerfile); run as
# `sh /start.sh <junk...>` those words are only this script's positional
# parameters, and mongod gets the argv it expects. `exec` replaces the shell
# in place: an execve(), no fork.
#
# --nounixsocket: nothing in the guest could connect over it, and mongod
# aborts if it cannot create the socket where it expects to.
# The cache bound keeps WiredTiger from sizing itself off the guest's total
# memory, which is mostly spoken for by the twice-resident rootfs.
exec /usr/bin/mongod --bind_ip_all --nounixsocket --wiredTigerCacheSizeGB 0.25
