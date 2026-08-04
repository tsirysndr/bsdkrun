# frozen_string_literal: true

module Bsdkrun
  # A machine as reported by +bsdkrun ps --json+.
  #
  # @!attribute [r] id
  #   @return [String]
  # @!attribute [r] name
  #   @return [String, nil] DNS name on a network, or nil if unnamed.
  # @!attribute [r] status
  #   @return [String] "running" or "exited" (derived from +running+).
  SandboxInfo = Data.define(
    :id, :name, :image, :kind, :command, :status, :running, :exit_code,
    :pid, :detached, :cpus, :mem, :volume, :state_dir, :network, :net_ip,
    :created_at, :finished_at
  ) do
    # Map a +ps --json+ row (String keys) to a typed instance.
    # @param row [Hash]
    # @return [SandboxInfo]
    def self.from_row(row)
      running = !!row["running"]
      new(
        id: row["id"].to_s,
        name: row["name"],
        image: row["image"].to_s,
        kind: row["kind"].to_s,
        command: (row["command"] || "").to_s,
        status: running ? "running" : "exited",
        running: running,
        exit_code: to_i_or_nil(row["exit_code"]),
        pid: to_i_or_nil(row["pid"]),
        detached: !!row["detached"],
        cpus: row["cpus"].to_i,
        mem: row["mem"].to_i,
        volume: row["volume"],
        state_dir: row["state_dir"].to_s,
        network: row["network"],
        net_ip: row["net_ip"],
        created_at: row["created_at"].to_i,
        finished_at: to_i_or_nil(row["finished_at"])
      )
    end

    def self.to_i_or_nil(value)
      value.nil? ? nil : value.to_i
    end
  end

  # An image as reported by +bsdkrun images --json+.
  ImageInfo = Data.define(:id, :reference, :digest, :size, :rootfs, :created_at) do
    # @param row [Hash]
    # @return [ImageInfo]
    def self.from_row(row)
      new(
        id: row["id"].to_s,
        reference: row["reference"].to_s,
        digest: row["digest"].to_s,
        size: row["size"].to_i,
        rootfs: row["rootfs"].to_s,
        created_at: row["created_at"].to_i
      )
    end
  end

  # A volume as reported by +bsdkrun volume ls --json+.
  VolumeInfo = Data.define(:name, :guest, :base, :path, :size, :created_at, :tracked) do
    # @param row [Hash]
    # @return [VolumeInfo]
    def self.from_row(row)
      created = row["created_at"]
      new(
        name: row["name"].to_s,
        guest: row["guest"],
        base: row["base"],
        path: row["path"].to_s,
        size: row["size"].to_s,
        created_at: created.nil? ? nil : created.to_i,
        tracked: !!row["tracked"]
      )
    end
  end

  # A global network as reported by +bsdkrun network ls --json+.
  NetworkInfo = Data.define(:name, :subnet, :gateway, :members, :running, :up, :created_at) do
    # @param row [Hash]
    # @return [NetworkInfo]
    def self.from_row(row)
      created = row["created_at"]
      new(
        name: row["name"].to_s,
        subnet: row["subnet"].to_s,
        gateway: row["gateway"].to_s,
        members: (row["members"] || 0).to_i,
        running: (row["running"] || 0).to_i,
        up: !!row["up"],
        created_at: created.nil? ? nil : created.to_i
      )
    end
  end

  # The captured result of running a command in a guest via {Sandbox#exec}.
  #
  # @!attribute [r] stdout
  #   @return [String]
  # @!attribute [r] stderr
  #   @return [String]
  # @!attribute [r] exit_code
  #   @return [Integer]
  # @!attribute [r] command
  #   @return [String] a human label for the command.
  Result = Data.define(:stdout, :stderr, :exit_code, :command) do
    # @return [Boolean] whether the command succeeded (exit 0).
    def ok?
      exit_code.zero?
    end

    # @return [String] stdout with trailing newlines trimmed — the common case.
    def text
      stdout.sub(/\n+\z/, "")
    end

    # @return [Object] stdout parsed as JSON.
    def json
      require "json"
      JSON.parse(stdout)
    end

    # @return [Array<String>] non-empty stdout lines.
    def lines
      stdout.split("\n").reject(&:empty?)
    end

    # @raise [CommandFailed] if the command exited non-zero.
    # @return [self]
    def throw_if_failed!
      unless ok?
        raise CommandFailed.new(
          exit_code: exit_code, stdout: stdout, stderr: stderr, command: command
        )
      end
      self
    end
  end
end
