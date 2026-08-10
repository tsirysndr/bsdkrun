# The entrypoint, in place of `bin/rails server -b 0.0.0.0`.
#
# The application's ARGV is not only what bsdkrun was given: libkrun appends
# its own hints to the kernel command line (earlycon=..., /tsi_hijack, a
# second `--`) and they arrive here, past the `--` stop sequence, as
# arguments. Thor -- the CLI layer under `rails server` -- parses ARGV and
# aborts on words it does not recognise. So: clear ARGV, then do exactly what
# bin/rails + `rails server` would have done, passing the server its options
# explicitly.
Dir.chdir(__dir__)
ARGV.clear

APP_PATH = File.expand_path("config/application", __dir__)
require_relative "config/boot"
require "rails/command"
Rails::Command.invoke "server", ["--binding=0.0.0.0", "--port=3000"]
