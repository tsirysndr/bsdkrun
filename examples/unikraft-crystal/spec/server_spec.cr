# A smoke test of the real thing: the server listens at load, so the spec
# runs it as a child — the same binary boundary the unikernel e2e asserts.
require "spec"
require "http/client"
require "json"

describe "server" do
  it "serves the greeting JSON" do
    child = Process.new("crystal", ["run", "src/server.cr"],
      output: Process::Redirect::Close, error: Process::Redirect::Close)
    begin
      response = nil
      60.times do
        begin
          response = HTTP::Client.get("http://127.0.0.1:8080/")
          break
        rescue IO::Error | Socket::ConnectError
          sleep 0.5.seconds
        end
      end
      response.should_not be_nil
      body = JSON.parse(response.not_nil!.body)
      body["message"].as_s.should eq("Hello from Crystal on Unikraft!")
    ensure
      child.terminate rescue nil
    end
  end
end
