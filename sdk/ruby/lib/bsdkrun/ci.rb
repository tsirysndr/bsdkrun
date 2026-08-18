# frozen_string_literal: true

require "fileutils"
require "json"
require "tmpdir"

module Bsdkrun
  # CI workflows defined in code instead of YAML.
  #
  # The builder produces exactly the file +bsdkrun ci+ (and tangled's spindle)
  # consumes — {CIWorkflow#yaml} is that file, {CIWorkflow#save} commits it to
  # +.tangled/workflows/+, and {CIWorkflow#run} executes it in a microVM
  # without a file ever touching the repository:
  #
  #   Bsdkrun.workflow("test")
  #     .on_push("main")
  #     .deps("ruby", "bundler")
  #     .env("CI_FROM", "sdk")
  #     .step("install", "bundle install")
  #     .step("test", "bundle exec rspec")
  #     .run
  #
  # Code is the source of truth and YAML the wire format, in that order —
  # which is why +save+ writes a generated-file header: a hand-edit there will
  # be overwritten by the next +save+.
  class CIWorkflow
    def initialize(name)
      @name = name
      @engine = "nixery"
      @when = []
      @deps = {}
      @env = {}
      @steps = []
      @clone_depth = nil
      @clone_skip = false
    end

    # Override the engine (+nixery+ by default).
    def engine(engine)
      @engine = engine
      self
    end

    # Add a push trigger for the given branches.
    def on_push(*branches)
      @when << [["push"], branches]
      self
    end

    # Add a pull_request trigger targeting the given branches.
    def on_pull_request(*branches)
      @when << [["pull_request"], branches]
      self
    end

    # Add a trigger with explicit events.
    def on(events, *branches)
      @when << [events, branches]
      self
    end

    # Add nixpkgs dependencies — the toolchain the steps run against.
    def deps(*packages)
      (@deps["nixpkgs"] ||= []).concat(packages)
      self
    end

    # Add dependencies from a custom registry (a flake reference).
    def deps_from(registry, *packages)
      (@deps[registry] ||= []).concat(packages)
      self
    end

    # Set a workflow-level environment variable.
    def env(key, value)
      @env[key] = value
      self
    end

    # Append a step; steps run serially in one VM, from the workspace root.
    def step(name, command, env: nil)
      @steps << { name: name, command: command, env: env || {} }
      self
    end

    # Set the clone depth (default 1).
    def clone_depth(depth)
      @clone_depth = depth
      self
    end

    # Skip the checkout entirely.
    def skip_clone
      @clone_skip = true
      self
    end

    # The workflow file name {#save} writes: +<name>.yml+.
    def file_name
      @name.match?(/\.ya?ml\z/) ? @name : "#{@name}.yml"
    end

    # Render the workflow file. Scalars are emitted as JSON strings — valid
    # YAML by construction — and commands as literal blocks when safe, so the
    # SDK needs no YAML dependency.
    def yaml
      out = []
      q = ->(s) { JSON.generate(s) }

      unless @when.empty?
        out << "when:"
        @when.each do |events, branches|
          out << "  - event: [#{events.map(&q).join(', ')}]"
          if branches.length == 1
            out << "    branch: #{q.call(branches[0])}"
          elsif branches.length > 1
            out << "    branch: [#{branches.map(&q).join(', ')}]"
          end
        end
        out << ""
      end

      out << "engine: #{@engine}"

      unless @deps.empty?
        out << "" << "dependencies:"
        @deps.keys.sort.each do |reg|
          out << "  #{q.call(reg)}:"
          @deps[reg].each { |p| out << "    - #{q.call(p)}" }
        end
      end

      unless @env.empty?
        out << "" << "environment:"
        @env.keys.sort.each { |k| out << "  #{k}: #{q.call(@env[k])}" }
      end

      if @clone_skip || @clone_depth
        out << "" << "clone:"
        out << "  skip: true" if @clone_skip
        out << "  depth: #{@clone_depth}" if @clone_depth
      end

      out << "" << "steps:"
      @steps.each do |s|
        out << "  - name: #{q.call(s[:name])}"
        # Literal blocks read well in a committed file, but cannot carry
        # trailing spaces or carriage returns byte-for-byte; fall back to a
        # JSON string rather than silently altering the command.
        block_safe = !s[:command].empty? &&
                     !s[:command].include?("\r") &&
                     s[:command].split("\n", -1).all? { |l| l == l.sub(/ +\z/, "") }
        if block_safe
          out << "    command: |"
          s[:command].sub(/\n+\z/, "").split("\n", -1).each { |l| out << "      #{l}" }
        else
          out << "    command: #{q.call(s[:command])}"
        end
        next if s[:env].empty?

        out << "    environment:"
        s[:env].keys.sort.each { |k| out << "      #{k}: #{q.call(s[:env][k])}" }
      end
      "#{out.join("\n")}\n"
    end

    # Write into +<repo>/.tangled/workflows/+ and return the path.
    def save(repo)
      dir = File.join(repo, ".tangled", "workflows")
      FileUtils.mkdir_p(dir)
      path = File.join(dir, file_name)
      File.write(
        path,
        "# Generated by the bsdkrun SDK — edit the code that save()d it instead.\n#{yaml}"
      )
      path
    end

    # Execute the workflow in a microVM, streaming output. The YAML never
    # touches the repository — it goes to a temp file and +bsdkrun ci run -f+.
    # Raises {CommandFailed} when a step fails.
    def run(dir: nil)
      Dir.mktmpdir("bsdkrun-ci-") do |tmp|
        file = File.join(tmp, file_name)
        File.write(file, yaml)
        args = ["ci", "run", "-f", file]
        args += ["-w", dir] if dir
        ok = Process.spawn_interactive(args)
        unless ok
          raise CommandFailed.new(
            exit_code: 1, stdout: "", stderr: "workflow #{@name} failed",
            command: "bsdkrun ci run"
          )
        end
      end
      nil
    end
  end

  # Start a CI workflow definition.
  def self.workflow(name)
    CIWorkflow.new(name)
  end
end
