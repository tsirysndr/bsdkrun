# frozen_string_literal: true

require "json"

module Bsdkrun
  # A handle to a running (or stopped) bsdkrun microVM.
  #
  # Create one with {Sandbox.create}, reconnect with {Sandbox.get}, or enumerate
  # with {Sandbox.list}.
  #
  # @example
  #   box = Bsdkrun::Sandbox.create(os: "linux", image: "alpine")
  #   box.exec(["uname", "-a"]).text
  #   box.stop
  class Sandbox
    ID_RE = /\A[0-9a-f]{6,}\z/
    SSH_PORT_RE = /ssh -p (\d+)/
    private_constant :ID_RE, :SSH_PORT_RE

    # The machine's Docker-style short id.
    # @return [String]
    attr_reader :id

    # Host port forwarded to the guest's SSH, if the boot banner reported one.
    # @return [Integer, nil]
    attr_reader :ssh_port

    # @param id [String]
    # @param ssh_port [Integer, nil]
    def initialize(id, ssh_port: nil)
      @id = id
      @ssh_port = ssh_port
    end

    # Read and write files in the guest.
    # @return [FileSystem]
    def fs
      @fs ||= FileSystem.new(@id)
    end

    class << self
      # Boot a new microVM and return a handle to it.
      #
      # Accepts create options as a keyword list or a Hash; discriminated on
      # +:os+ ("linux", "freebsd", "netbsd", "firmware", "kernel").
      #
      # @param opts [Hash]
      # @return [Sandbox]
      # @raise [CommandFailed] if boot fails or no machine id is printed.
      def create(opts = {}, **kwargs)
        opts = normalize(opts.merge(kwargs))
        args = Args.build_create_args(opts)
        res = Process.run(args, log_level: opts.fetch(:log_level, 1))
        if res.exit_code != 0
          raise CommandFailed.new(
            exit_code: res.exit_code, stdout: res.stdout, stderr: res.stderr,
            command: "bsdkrun create"
          )
        end
        # Detached runs print just the machine id on stdout.
        id = res.stdout.split("\n").map(&:strip).select { |l| l.match?(ID_RE) }.last
        unless id
          raise CommandFailed.new(
            exit_code: res.exit_code, stdout: res.stdout, stderr: res.stderr,
            command: "bsdkrun create (no machine id in output)"
          )
        end
        match = res.stderr.match(SSH_PORT_RE)
        new(id, ssh_port: match && match[1].to_i)
      end

      # Reconnect to an existing machine by id (a unique prefix is enough).
      #
      # @param id [String]
      # @return [Sandbox]
      # @raise [SandboxNotFound]
      def get(id)
        row = list(all: true).find { |m| m.id == id || m.id.start_with?(id) || m.name == id }
        raise SandboxNotFound, id unless row

        new(row.id)
      end

      # List machines. +all: true+ includes exited ones (default running only).
      #
      # @param all [Boolean]
      # @return [Array<SandboxInfo>]
      def list(all: false)
        args = ["ps", "--json"]
        args << "--all" if all
        res = Process.run!(args, label: "bsdkrun ps")
        rows = JSON.parse(res.stdout.empty? ? "[]" : res.stdout)
        rows.map { |row| SandboxInfo.from_row(row) }
      end

      # Symbolize keys and normalize a nested +:net+ hash.
      # @!visibility private
      def normalize(opts)
        h = opts.each_with_object({}) { |(k, v), acc| acc[k.to_sym] = v }
        h[:net] = h[:net].each_with_object({}) { |(k, v), acc| acc[k.to_sym] = v } if h[:net].is_a?(Hash)
        h
      end
    end

    # Run a command in the guest through its exec agent.
    #
    # +command+ may be an Array (argv, no shell parsing) or a String program
    # name; with a String, +args:+ supplies its arguments.
    #
    # @param command [String, Array<String>]
    # @param args [Array<String>] arguments when +command+ is a bare String.
    # @param env [Hash] environment variables (+-e K=V+).
    # @param tty [Boolean] allocate a pseudo-TTY in the guest (+-t+).
    # @param stdin [String, nil] data piped to the command's stdin.
    # @param cwd [String, nil] working directory (emulated via +sh -c 'cd …'+).
    # @param throw_on_error [Boolean] raise {CommandFailed} on a non-zero exit.
    # @param log_level [Integer] per-command bsdkrun log level.
    # @param on_stdout [Proc, nil] called with each stdout chunk as it arrives.
    # @param on_stderr [Proc, nil] called with each stderr chunk as it arrives.
    # @return [Result]
    def exec(command, args: [], env: {}, tty: false, stdin: nil, cwd: nil,
             throw_on_error: false, log_level: 0, on_stdout: nil, on_stderr: nil)
      argv = command.is_a?(Array) ? command.dup : [command, *args]

      if cwd
        # Emulate a working directory: cd, drop it, then exec the real argv.
        argv = ["/bin/sh", "-c", 'cd "$1" && shift && exec "$@"', "sh", cwd, *argv]
      end

      cli = ["exec"]
      cli << "-t" if tty
      env.each { |k, v| cli.push("-e", "#{k}=#{v}") }
      cli.push(@id, *argv)

      res = Process.run(cli, stdin: stdin, log_level: log_level,
                        on_stdout: on_stdout, on_stderr: on_stderr)
      result = Result.new(
        stdout: res.stdout, stderr: res.stderr, exit_code: res.exit_code,
        command: "exec #{argv.join(' ')}"
      )
      result.throw_if_failed! if throw_on_error
      result
    end

    # Vercel-Sandbox-style alias for {#exec}: a program plus its args.
    #
    # @param command [String]
    # @param args [Array<String>]
    # @return [Result]
    def run_command(command, args = [], **opts)
      exec(command, args: args, **opts)
    end

    # Read the machine's console log.
    #
    # @param boot [Boolean] show bsdkrun's own boot log instead of the console.
    # @return [String]
    def logs(boot: false)
      args = ["logs"]
      args << "--boot" if boot
      args << @id
      Process.run(args).stdout
    end

    # Attach an interactive shell to the machine (inherits the terminal).
    # @return [Boolean] true if the shell exited zero.
    def shell
      Process.spawn_interactive(["shell", @id])
    end

    # Fetch this machine's current status row, or nil if it's gone.
    # @return [SandboxInfo, nil]
    def status
      Sandbox.list(all: true).find { |m| m.id == @id }
    end

    # Whether the machine is currently running.
    # @return [Boolean]
    def running?
      s = status
      s ? s.running : false
    end

    # Stop the machine. BSD guests are cleanly powered off; Linux is SIGTERM'd.
    # @return [void]
    def stop
      lifecycle(["stop", @id], "bsdkrun stop")
    end

    # Restart a stopped machine in place (same id, disk/rootfs). Boots detached.
    # @return [void]
    def start
      lifecycle(["start", @id], "bsdkrun start")
    end

    # Remove the machine and its state. +force+ stops it first if running.
    # @param force [Boolean]
    # @return [void]
    def remove(force: false)
      args = ["rm"]
      args << "--force" if force
      args << @id
      lifecycle(args, "bsdkrun rm")
    end

    # Change the recorded vCPU / RAM. Applies on the next {#start}.
    # @param cpus [Integer, nil]
    # @param mem [Integer, nil]
    # @return [void]
    def update(cpus: nil, mem: nil)
      args = ["update", @id]
      args.push("--cpus", cpus.to_s) unless cpus.nil?
      args.push("--mem", mem.to_s) unless mem.nil?
      lifecycle(args, "bsdkrun update")
    end

    # Join or switch this machine to a global network. Applies on next {#start}.
    # @param network [String]
    # @return [void]
    def connect_network(network)
      lifecycle(["network", "connect", @id, network], "bsdkrun network connect")
    end

    # Detach this machine from its network. Applies on next {#start}.
    # @return [void]
    def disconnect_network
      lifecycle(["network", "disconnect", @id], "bsdkrun network disconnect")
    end

    # Install SSH keys in the guest via the agent (+ssh setup+). With no keys,
    # the CLI installs your local +~/.ssh/*.pub+.
    #
    # @param user [String, nil] target user (default root).
    # @param key [String, Array<String>, nil] literal key(s) or +.pub+ path(s).
    # @return [Result]
    def ssh_setup(user: nil, key: nil)
      action = ["setup"]
      action.push("--user", user) if user
      Array(key).each { |k| action.push("--key", k) }
      agent("ssh", action)
    end

    # Put the guest on your tailnet (+tailscale setup+).
    #
    # @param authkey [String, nil] tailnet auth key (sent as +TS_AUTHKEY+).
    # @param hostname [String, nil] machine name on the tailnet.
    # @param args [Array<String>] extra args passed through to +tailscale up+.
    # @return [Result]
    def tailscale_up(authkey: nil, hostname: nil, args: [])
      action = ["setup"]
      action.push("--hostname", hostname) if hostname
      action.concat(args)
      agent("tailscale", action, env: authkey ? { "TS_AUTHKEY" => authkey } : {})
    end

    private

    # Run a fire-and-forget lifecycle CLI command, raising on failure.
    def lifecycle(args, label)
      Process.run!(args, label: label)
      nil
    end

    # Run an in-guest agent CLI family (+ssh+, +tailscale+), raising on failure.
    def agent(family, action, env: {})
      res = Process.run([family, @id, *action], env: env)
      result = Result.new(
        stdout: res.stdout, stderr: res.stderr, exit_code: res.exit_code,
        command: "#{family} #{action.join(' ')}"
      )
      result.throw_if_failed!
    end
  end
end
