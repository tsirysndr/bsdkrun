# frozen_string_literal: true

require "net/http"
require "uri"
require "json"
require "base64"

require_relative "errors"
require_relative "types"
require_relative "ws_client"
require_relative "shell_session"

module Bsdkrun
  # A client that talks to a remote +bsdkrund+ daemon's GraphQL API directly
  # — HTTP for queries/mutations, a hand-rolled +graphql-transport-ws+ socket
  # for subscriptions — instead of shelling out to a local +bsdkrun+ binary
  # the way {Bsdkrun::Sandbox} does.
  #
  # The wire contract (URL/header shape, error mapping, subscription
  # protocol, field names) is locked to match the other bsdkrun SDKs
  # (TypeScript/Python/Elixir/Gleam) and the web frontend's
  # +web/src/lib/graphql.ts+ byte-for-byte — see that file for the reference
  # implementation this one mirrors.
  #
  # @example
  #   client = Bsdkrun::Client.from_env
  #   client.list.each { |m| puts m.id }
  #   result = client.exec(id, ["uname", "-a"])
  #   puts result.output
  class Client
    URL_ENV = "BSDKRUN_URL"
    TOKEN_ENV = "BSDKRUN_TOKEN"

    # The +Machine+ field selection shared by +machines+ / +machine+ — mirrors
    # +web/src/lib/api.ts+'s +MACHINE_FIELDS+ fragment exactly.
    MACHINE_FIELDS = <<~GQL.freeze
      id name image kind command status running exitCode pid detached
      cpus mem volume stateDir createdAt finishedAt network netIp
      ports { bind host guest }
    GQL

    # @return [String] the GraphQL endpoint URL (normalized).
    attr_reader :url

    # @param url [String] the daemon's URL, e.g. "http://host:50052" or a
    #   full "http://host:50052/graphql" — normalized either way.
    # @param token [String] the daemon's bearer token.
    def initialize(url:, token:)
      @url = self.class.normalize_url(url)
      @token = token.to_s
      @ws_mutex = Mutex.new
      @ws = nil
    end

    # Build a client from +BSDKRUN_URL+ / +BSDKRUN_TOKEN+.
    #
    # A host set without a token is an error, not a silent fallback —
    # mirrors +daemon/src/client.rs+'s +RemoteConfig::from_env+ (which uses
    # +BSDKRUN_HOST+/+BSDKRUN_TOKEN+ for the gRPC client; these are
    # GraphQL-specific env vars with a different URL shape, not aliases).
    #
    # @return [Client]
    # @raise [Error] if +BSDKRUN_URL+ is unset, or set without +BSDKRUN_TOKEN+.
    def self.from_env
      url = ENV[URL_ENV]
      raise Error, "#{URL_ENV} is not set; nothing to connect to" if url.nil? || url.strip.empty?

      token = ENV[TOKEN_ENV]
      if token.nil? || token.strip.empty?
        raise Error, "#{URL_ENV} is set but #{TOKEN_ENV} is not"
      end

      new(url: url, token: token)
    end

    # Normalize a user-supplied URL into a full GraphQL endpoint: trim, add
    # +http://+ if no scheme was given, strip trailing slashes, append
    # +/graphql+ unless the path already ends with it. Mirrors
    # +web/src/lib/connection.ts+'s +normalizeUrl+ exactly.
    #
    # @param input [String]
    # @return [String]
    def self.normalize_url(input)
      s = input.to_s.strip
      return s if s.empty?

      s = "http://#{s}" unless s.match?(%r{\Ahttps?://}i)
      s = s.sub(%r{/+\z}, "")
      s = "#{s}/graphql" unless s.match?(%r{/graphql\z}i)
      s
    end

    # Derive the websocket endpoint from the HTTP one: +http://+ -> +ws://+,
    # +https://+ -> +wss://+, trailing slashes on the path stripped, +/ws+
    # appended. Mirrors +web/src/lib/graphql.ts+'s +wsUrl+.
    #
    # @param http_url [String]
    # @return [String]
    def self.ws_url(http_url)
      uri = URI.parse(http_url)
      uri.scheme = uri.scheme == "https" ? "wss" : "ws"
      uri.path = "#{uri.path.to_s.sub(%r{/+\z}, "")}/ws"
      uri.to_s
    end

    # ---- HTTP transport: the escape hatch, and everything below is built on it ----

    # Run an arbitrary query or mutation. Every typed method on this class is
    # implemented in terms of this — it exists as a public escape hatch for
    # documents this SDK has no typed wrapper for yet.
    #
    # @param query [String] a GraphQL document.
    # @param variables [Hash]
    # @return [Hash] +body["data"]+ (String-keyed, as parsed by +JSON.parse+).
    # @raise [AuthError] on HTTP 401, or a GraphQL error with
    #   +extensions.code == "UNAUTHENTICATED"+.
    # @raise [GraphQLError] on transport failure, a non-JSON response, or any
    #   other GraphQL error.
    def request(query, variables = {})
      uri = URI.parse(@url)
      http = Net::HTTP.new(uri.host, uri.port)
      http.use_ssl = uri.scheme == "https"

      req = Net::HTTP::Post.new(uri.request_uri.empty? ? "/" : uri.request_uri)
      req["content-type"] = "application/json"
      req["authorization"] = "Bearer #{@token}"
      req.body = JSON.generate({ query: query, variables: variables })

      begin
        res = http.request(req)
      rescue StandardError => e
        raise GraphQLError, "cannot reach the bsdkrun daemon at #{@url} — #{e.message}"
      end

      raise AuthError if res.code.to_i == 401

      body = begin
        JSON.parse(res.body.to_s)
      rescue JSON::ParserError
        nil
      end
      raise GraphQLError, "the daemon returned a non-JSON response (#{res.code})" if body.nil?

      errors = body["errors"]
      if errors.is_a?(Array) && !errors.empty?
        first = errors.first
        message = first["message"].to_s
        code = first.is_a?(Hash) ? first.dig("extensions", "code") : nil
        raise AuthError, message if code == "UNAUTHENTICATED"
        raise GraphQLError.new(message, code)
      end

      body["data"]
    end

    # ---- subscriptions ----------------------------------------------------

    # Start a subscription over the shared websocket (opened lazily on first
    # use). See {WsClient#subscribe} for the exact queueing/ack semantics.
    #
    # @param query [String]
    # @param variables [Hash]
    # @param on_next [#call]
    # @param on_error [#call, nil]
    # @param on_complete [#call, nil]
    # @return [Proc] call to unsubscribe.
    def subscribe(query, variables = {}, on_next:, on_error: nil, on_complete: nil)
      ws.subscribe(query, variables, on_next: on_next, on_error: on_error, on_complete: on_complete)
    end

    # ---- lifecycle / listing -----------------------------------------------

    # @param all [Boolean] include stopped machines too.
    # @return [Array<SandboxInfo>]
    def list(all: false)
      data = request("query($all:Boolean!){ machines(all:$all){ #{MACHINE_FIELDS} } }", { all: all })
      (data["machines"] || []).map { |m| SandboxInfo.from_graphql(m) }
    end

    # @param id [String] id, name, or unique id prefix.
    # @return [SandboxInfo, nil] nil if no such machine exists.
    def get(id)
      data = request("query($id:String!){ machine(id:$id){ #{MACHINE_FIELDS} } }", { id: id })
      m = data["machine"]
      m && SandboxInfo.from_graphql(m)
    end

    # @param id [String]
    # @return [CommandResult]
    def stop(id)
      run_command_mutation(
        "stopMachine",
        "mutation($id:String!){ stopMachine(id:$id){ exitCode stdout stderr } }",
        { id: id }
      )
    end

    # @param id [String]
    # @return [CommandResult]
    def start(id)
      run_command_mutation(
        "startMachine",
        "mutation($id:String!){ startMachine(id:$id){ exitCode stdout stderr } }",
        { id: id }
      )
    end

    # @param ids [String, Array<String>]
    # @param force [Boolean]
    # @return [CommandResult]
    def remove(ids, force: false)
      run_command_mutation(
        "removeMachines",
        "mutation($ids:[String!]!,$force:Boolean!){ removeMachines(ids:$ids, force:$force){ exitCode stdout stderr } }",
        { ids: Array(ids), force: force }
      )
    end

    # @param id [String]
    # @param cpus [Integer, nil]
    # @param mem [Integer, nil]
    # @return [CommandResult]
    def update(id, cpus: nil, mem: nil)
      run_command_mutation(
        "updateMachine",
        "mutation($id:String!,$cpus:Int,$mem:Int){ updateMachine(id:$id, cpus:$cpus, mem:$mem){ exitCode stdout stderr } }",
        { id: id, cpus: cpus, mem: mem }
      )
    end

    # Snapshot a machine into a named flavor, like +docker commit+.
    # @param id [String]
    # @param name [String]
    # @param description [String]
    # @return [CommandResult]
    def commit(id, name, description: "")
      run_command_mutation(
        "commitMachine",
        "mutation($id:String!,$name:String!,$description:String!){ " \
        "commitMachine(id:$id, name:$name, description:$description){ exitCode stdout stderr } }",
        { id: id, name: name, description: description }
      )
    end

    # One-shot console log fetch. Use {#follow_logs} to stream instead.
    # @param id [String]
    # @param boot [Boolean] bsdkrun's own boot log instead of the guest console.
    # @return [String]
    def logs(id, boot: false)
      data = request("query($id:String!,$boot:Boolean!){ machineLogs(id:$id, boot:$boot) }",
                      { id: id, boot: boot })
      data["machineLogs"]
    end

    # Stream a machine's console log live.
    # @param id [String]
    # @param follow [Boolean]
    # @param boot [Boolean]
    # @yieldparam data [String] binary-safe decoded chunk.
    # @return [Proc] call to stop following.
    def follow_logs(id, follow: true, boot: false, &on_data)
      subscribe(
        "subscription($id:String!,$follow:Boolean!,$boot:Boolean!){ " \
        "machineLogs(id:$id, follow:$follow, boot:$boot){ dataBase64 exitCode } }",
        { id: id, follow: follow, boot: boot },
        on_next: lambda { |data|
          payload = data && data["machineLogs"]
          next unless payload && payload["dataBase64"]

          on_data&.call(Base64.decode64(payload["dataBase64"]))
        }
      )
    end

    # ---- booting ------------------------------------------------------------
    #
    # Six variants, one per daemon mutation — see +daemon/src/graphql.rs+ for
    # the exact input field names this transcribes (RunLinuxInput ~L384,
    # RunBsdInput ~L406, RunNanosInput ~L428, RunUnikraftInput ~L449,
    # RunOsvInput ~L467, RunFlavorInput ~L506, NetInput ~L360). Each accepts
    # an options Hash and/or keyword arguments, merged — the same convention
    # {Sandbox.create} uses — with snake_case Ruby keys mapped 1:1 to the
    # camelCase GraphQL fields on the wire.

    # @return [String] the new machine's id.
    def run_linux(opts = {}, **kwargs)
      o = merge_opts(opts, kwargs)
      input = {
        image: o.fetch(:image),
        cpus: o[:cpus],
        mem: o[:mem],
        net: net_input(o[:net]),
        volume: o[:volume],
        mounts: o[:mounts] || [],
        env: o[:env] || [],
        entrypoint: o[:entrypoint],
        initramfs: o[:initramfs] || false,
        kernel: o[:kernel],
        kernelVersion: o[:kernel_version],
        console: o[:console],
        repo: o[:repo],
        command: o[:command] || []
      }
      request("mutation($i:RunLinuxInput!){ runLinux(input:$i) }", { i: input })["runLinux"]
    end

    # @return [String] the new machine's id.
    def run_bsd(opts = {}, **kwargs)
      o = merge_opts(opts, kwargs)
      input = {
        os: bsd_os_enum(o.fetch(:os)),
        version: o[:version],
        cpus: o[:cpus],
        mem: o[:mem],
        net: net_input(o[:net]),
        volume: o[:volume],
        persist: o[:persist] || false,
        force: o[:force] || false,
        firmware: o[:firmware],
        attachDisk: o[:attach_disk] || [],
        diskSize: o[:disk_size],
        repo: o[:repo],
        command: o[:command] || []
      }
      request("mutation($i:RunBsdInput!){ runBsd(input:$i) }", { i: input })["runBsd"]
    end

    # No agent (no exec/shell), but it does have a root disk, so +persist:+
    # is the one disk option it takes.
    # @return [String] the new machine's id.
    def run_nanos(opts = {}, **kwargs)
      o = merge_opts(opts, kwargs)
      input = {
        image: o.fetch(:image),
        cpus: o[:cpus],
        mem: o[:mem],
        net: net_input(o[:net]),
        kernel: o[:kernel],
        cmdline: o[:cmdline],
        persist: o[:persist] || false
      }
      request("mutation($i:RunNanosInput!){ runNanos(input:$i) }", { i: input })["runNanos"]
    end

    # A unikernel has no disk and no agent, so no volume/persist/repo/command
    # fields — +mounts:+ (virtio-fs shares) is the exception, needing neither.
    # @return [String] the new machine's id.
    def run_unikraft(opts = {}, **kwargs)
      o = merge_opts(opts, kwargs)
      input = {
        path: o[:path],
        cpus: o[:cpus],
        mem: o[:mem],
        net: net_input(o[:net]),
        cmdline: o[:cmdline],
        initramfs: o[:initramfs],
        mounts: o[:mounts] || []
      }
      request("mutation($i:RunUnikraftInput!){ runUnikraft(input:$i) }", { i: input })["runUnikraft"]
    end

    # Like Nanos, no agent, but it does have a root filesystem, so the disk
    # options apply — +disk:+ in particular, how an x86_64 guest gets a
    # filesystem (its loader ELF is kernel only).
    # @return [String] the new machine's id.
    def run_osv(opts = {}, **kwargs)
      o = merge_opts(opts, kwargs)
      input = {
        image: o.fetch(:image),
        cpus: o[:cpus],
        mem: o[:mem],
        net: net_input(o[:net]),
        cmdline: o[:cmdline],
        disk: o[:disk],
        noDisk: o[:no_disk] || false,
        attachDisk: o[:attach_disk] || [],
        gic: o[:gic],
        persist: o[:persist] || false,
        volume: o[:volume]
      }
      request("mutation($i:RunOsvInput!){ runOsv(input:$i) }", { i: input })["runOsv"]
    end

    # @return [String] the new machine's id.
    def run_flavor(opts = {}, **kwargs)
      o = merge_opts(opts, kwargs)
      input = {
        name: o.fetch(:name),
        cpus: o[:cpus],
        mem: o[:mem],
        ports: o[:ports] || [],
        volume: o[:volume],
        repo: o[:repo]
      }
      request("mutation($i:RunFlavorInput!){ runFlavor(input:$i) }", { i: input })["runFlavor"]
    end

    # ---- exec / interactive shell ------------------------------------------

    # Run a command to completion and collect its output. Implemented as the
    # three-operation sequence +daemon/README.md+ documents: +openShell+
    # (with a +command:+, so the session runs it instead of a login shell),
    # THEN subscribe to +shellOutput+ (so nothing written in between is
    # lost), THEN wait for an exit code. +closeShell+ runs in an +ensure+ so
    # it happens whether the wait succeeded, failed, or raised.
    #
    # @param id [String] machine id.
    # @param command [Array<String>] argv.
    # @param env [Hash, Array<String>, nil] +"K=V"+ pairs, or a Hash of them.
    # @return [ExecResult]
    def exec(id, command, env: nil)
      data = request(
        "mutation($m:String!,$c:[String!]!,$e:[String!]!,$r:Int!,$k:Int!){ " \
        "openShell(machineId:$m, command:$c, env:$e, rows:$r, cols:$k){ id } }",
        { m: id, c: Array(command), e: env_to_list(env), r: 24, k: 80 }
      )
      session_id = data["openShell"]["id"]

      output = +"".b
      exit_code = nil
      done = Queue.new

      unsubscribe = subscribe(
        "subscription($s:String!){ shellOutput(sessionId:$s){ dataBase64 exitCode } }",
        { s: session_id },
        on_next: lambda { |d|
          payload = d && d["shellOutput"]
          next unless payload

          output << Base64.decode64(payload["dataBase64"]) if payload["dataBase64"]
          unless payload["exitCode"].nil?
            exit_code = payload["exitCode"]
            done << :done
          end
        },
        on_error: ->(e) { done << e },
        on_complete: -> { done << :done }
      )

      begin
        result = done.pop
        raise result if result.is_a?(Exception)
      ensure
        unsubscribe.call
        begin
          request("mutation($s:String!){ closeShell(sessionId:$s) }", { s: session_id })
        rescue GraphQLError
          # closeShell is idempotent server-side; a request failure here
          # (already gone, machine removed, etc.) must not mask the actual
          # exec result/exception above — same as the other SDKs' Client#exec.
          nil
        end
      end

      ExecResult.new(exit_code: exit_code, output: output)
    end

    # Open a live interactive session. Unlike {#exec}, this returns
    # immediately with a handle whose {ShellSession#on_output} /
    # {ShellSession#on_exit} callbacks fire as output arrives.
    #
    # @param id [String] machine id.
    # @param command [Array<String>, nil] nil opens a login shell.
    # @param env [Hash, Array<String>, nil]
    # @param rows [Integer]
    # @param cols [Integer]
    # @return [ShellSession]
    def shell(id, command: nil, env: nil, rows: 24, cols: 80)
      data = request(
        "mutation($m:String!,$c:[String!]!,$e:[String!]!,$r:Int!,$k:Int!){ " \
        "openShell(machineId:$m, command:$c, env:$e, rows:$r, cols:$k){ id } }",
        { m: id, c: command.nil? ? [] : Array(command), e: env_to_list(env), r: rows, k: cols }
      )
      session_id = data["openShell"]["id"]
      session = ShellSession.new(client: self, id: session_id)

      unsubscribe = subscribe(
        "subscription($s:String!){ shellOutput(sessionId:$s){ dataBase64 exitCode } }",
        { s: session_id },
        on_next: lambda { |d|
          payload = d && d["shellOutput"]
          next unless payload

          session.deliver_output(Base64.decode64(payload["dataBase64"])) if payload["dataBase64"]
          session.deliver_exit(payload["exitCode"]) unless payload["exitCode"].nil?
        },
        on_error: ->(_e) { session.deliver_exit(nil) },
        on_complete: -> {}
      )
      session.unsubscribe = unsubscribe
      session
    end

    private

    def ws
      @ws_mutex.synchronize { @ws ||= WsClient.new(ws_url: self.class.ws_url(@url), token: @token) }
    end

    def run_command_mutation(field, query, variables)
      r = request(query, variables)[field]
      CommandResult.new(exit_code: r["exitCode"].to_i, stdout: r["stdout"].to_s, stderr: r["stderr"].to_s)
    end

    # Symbolize keys and merge a positional options Hash with keyword
    # arguments — mirrors {Args.normalize}, which {Sandbox.create} uses.
    def merge_opts(opts, kwargs)
      opts.merge(kwargs).each_with_object({}) { |(k, v), acc| acc[k.to_sym] = v }
    end

    # @param net [Hash, nil] +:no_net+/+:ports+/+:mac+/+:network+/+:name+.
    # @return [Hash, nil] a +NetInput+-shaped wire hash.
    def net_input(net)
      return nil unless net

      n = net.each_with_object({}) { |(k, v), acc| acc[k.to_sym] = v }
      {
        noNet: n[:no_net] || n[:noNet] || false,
        ports: n[:ports] || [],
        mac: n[:mac],
        network: n[:network],
        name: n[:name]
      }
    end

    def bsd_os_enum(os)
      case os.to_s.downcase
      when "freebsd" then "FREEBSD"
      when "netbsd" then "NETBSD"
      else raise ArgumentError, "unknown BSD os: #{os.inspect}"
      end
    end

    # @param env [Hash, Array<String>, nil]
    # @return [Array<String>] "K=V" pairs, as +openShell+'s +env:+ field wants.
    def env_to_list(env)
      return [] if env.nil?
      return env if env.is_a?(Array)

      env.map { |k, v| "#{k}=#{v}" }
    end
  end
end
