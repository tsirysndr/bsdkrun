# A Crystal HTTP service, to prove it runs as a Unikraft unikernel.
require "http/server"
require "json"

PORT = 8080

server = HTTP::Server.new do |context|
  context.response.content_type = "application/json"
  context.response.print({
    "message" => "Hello from Crystal on Unikraft!",
    "crystal" => Crystal::VERSION,
    "path"    => context.request.path,
  }.to_json)
end

puts "Crystal #{Crystal::VERSION} listening on :#{PORT}"
STDOUT.flush
server.listen("0.0.0.0", PORT)
