# A smoke test of the real thing: the server listens at load, so the test
# runs it as a child — the same binary boundary the unikernel e2e asserts.
require "minitest/autorun"
require "net/http"

class ServerTest < Minitest::Test
  def test_greets
    pid = spawn(RbConfig.ruby, "server.rb", out: File::NULL, err: File::NULL)
    body = nil
    50.times do
      begin
        body = Net::HTTP.get(URI("http://127.0.0.1:8080/"))
        break
      rescue Errno::ECONNREFUSED, Errno::ECONNRESET, EOFError
        sleep 0.1
      end
    end
    assert_equal "Hello, World!", body&.strip
  ensure
    begin
      Process.kill("TERM", pid) if pid
    rescue Errno::ESRCH
    end
  end
end
