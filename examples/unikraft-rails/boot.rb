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

# The ruby image the rootfs was assembled from exports
# GEM_HOME=/usr/local/bundle, and that variable -- not any file convention --
# is how rubygems finds the gems `rails new` installed. The guest boots with
# only the four variables the Kraftfile bakes (PATH, LD_LIBRARY_PATH, HOME,
# PWD), so without this line bundler resolves the Gemfile against the default
# gem path and reports every gem missing: "Could not find rails-7.1.6, ...
# Run `bundle install`". Gem.clear_paths makes rubygems re-read the
# environment in case it was touched before this line ran.
ENV["GEM_HOME"] ||= "/usr/local/bundle"
Gem.clear_paths

# Ruby takes its default external encoding from the locale, and the guest has
# no LANG -- the Kraftfile bakes four environment variables and that is not
# one of them -- so it settles on US-ASCII. Rails' own templates and helpers
# are UTF-8, so the first page render dies with
#
#   ActionView::Template::Error (invalid byte sequence in UTF-8)
#
# and the server answers 500 while looking perfectly healthy in its log.
# Setting the default here is enough because it applies to files opened
# afterwards, and every template is read later than this.
ENV["LANG"] ||= "C.UTF-8"
Encoding.default_external = Encoding::UTF_8
Encoding.default_internal = Encoding::UTF_8

APP_PATH = File.expand_path("config/application", __dir__)
require_relative "config/boot"
require "rails/command"
Rails::Command.invoke "server", ["--binding=0.0.0.0", "--port=3000"]
