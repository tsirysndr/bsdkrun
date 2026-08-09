# An ordinary Sinatra app. Nothing here knows it is running on a unikernel.
require "sinatra"

# WEBrick, because it is pure Ruby: puma and thin ship native extensions that
# would each need their own libraries collected into the image.
set :server, "webrick"
# The host reaches this through bsdkrun's forwarded port, so binding to
# loopback would make it unreachable from outside the VM.
set :bind, "0.0.0.0"
set :port, 4567

get "/" do
  "hello from Sinatra on OSv\n"
end

get "/info" do
  "ruby #{RUBY_VERSION} on #{RUBY_PLATFORM}\n"
end
