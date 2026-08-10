#!/bin/sh
# Not an init script -- an argv filter. libkrun appends its own words to the
# command line and they land in the boot argv (see the Dockerfile); run as
# `sh /start.sh <junk...>` those words are only this script's positional
# parameters, and mariadbd gets the argv it expects. `exec` replaces the shell
# in place: an execve(), no fork.
#
# The rest of the server's configuration is /etc/my.cnf inside the image.
exec /usr/sbin/mariadbd --user=root
