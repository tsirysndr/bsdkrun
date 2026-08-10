#!/bin/sh
# Not an init script -- an argv filter. libkrun appends its own words to the
# command line and they land in the boot argv (see the Dockerfile); run as
# `sh /start.sh <junk...>` those words are only this script's positional
# parameters, and apache2 gets the argv it expects. `exec` replaces the shell
# in place: an execve(), no fork.
#
# -X: single-process debug mode (see the Dockerfile) -- the only MPM mode
# that never forks a worker.
exec /usr/sbin/apache2 -X -f /etc/apache2/apache2.conf
