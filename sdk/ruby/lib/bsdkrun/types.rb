# frozen_string_literal: true

module Bsdkrun
  # A host->guest TCP port forward, as reported by +bsdkrun ps --json+.
  #
  # @!attribute [r] bind
  #   @return [String] host interface, e.g. "127.0.0.1" or "0.0.0.0".
  # @!attribute [r] host
  #   @return [Integer] host port.
  # @!attribute [r] guest
  #   @return [Integer] guest port.
  PortForward = Data.define(:bind, :host, :guest) do
    # @param row [Hash]
    # @return [PortForward]
    def self.from_row(row)
      new(
        bind: row["bind"].to_s,
        host: row["host"].to_i,
        guest: row["guest"].to_i
      )
    end
  end

  # A machine as reported by +bsdkrun ps --json+.
  #
  # @!attribute [r] id
  #   @return [String]
  # @!attribute [r] name
  #   @return [String, nil] DNS name on a network, or nil if unnamed.
  # @!attribute [r] status
  #   @return [String] "running" or "exited" (derived from +running+).
  # @!attribute [r] ports
  #   @return [Array<PortForward>] host<->guest TCP port forwards.
  SandboxInfo = Data.define(
    :id, :name, :image, :kind, :command, :status, :running, :exit_code,
    :pid, :detached, :cpus, :mem, :volume, :state_dir, :network, :net_ip,
    :created_at, :finished_at, :ports, :origin
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
        finished_at: to_i_or_nil(row["finished_at"]),
        ports: (row["ports"] || []).map { |p| PortForward.from_row(p) },
        origin: row["origin"]
      )
    end

    def self.to_i_or_nil(value)
      value.nil? ? nil : value.to_i
    end

    # Map a GraphQL +Machine+ object (String keys, camelCase — it comes from
    # +JSON.parse+ on the daemon's response, not the CLI) to a typed instance.
    # Sibling to {from_row}; same fields, different source and casing.
    #
    # @param m [Hash] a +MACHINE_FIELDS+-shaped hash, e.g. from
    #   {Bsdkrun::Client#list} / {Bsdkrun::Client#get}.
    # @return [SandboxInfo]
    def self.from_graphql(m)
      running = !!m["running"]
      new(
        id: m["id"].to_s,
        name: m["name"],
        image: m["image"].to_s,
        kind: m["kind"].to_s,
        command: (m["command"] || "").to_s,
        status: m["status"].to_s,
        running: running,
        exit_code: to_i_or_nil(m["exitCode"]),
        pid: to_i_or_nil(m["pid"]),
        detached: !!m["detached"],
        cpus: m["cpus"].to_i,
        mem: m["mem"].to_i,
        volume: m["volume"],
        state_dir: m["stateDir"].to_s,
        network: m["network"],
        net_ip: m["netIp"],
        created_at: m["createdAt"].to_i,
        finished_at: to_i_or_nil(m["finishedAt"]),
        ports: (m["ports"] || []).map { |p| PortForward.from_row(p) },
        origin: m["origin"]
      )
    end
  end

  # A machine snapshot: one machine's disk state, captured under a name.
  #
  # A copy-on-write clone rather than a memory image — the files the guest
  # wrote, not what it was executing. {Bsdkrun::Client#branch} boots a new
  # machine from one; {Bsdkrun::Client#restore} puts one back.
  SnapshotInfo = Data.define(
    :id, :name, :machine_id, :machine_name, :kind, :image, :path, :parent,
    :description, :cpus, :mem, :ports, :size, :created_at
  ) do
    # Map a GraphQL +Snapshot+ (camelCase) to a typed instance.
    # @param s [Hash]
    # @return [SnapshotInfo]
    def self.from_graphql(s)
      new(
        id: s["id"].to_s,
        name: s["name"].to_s,
        machine_id: s["machineId"].to_s,
        machine_name: (s["machineName"] || "").to_s,
        kind: s["kind"].to_s,
        image: (s["image"] || "").to_s,
        path: (s["path"] || "").to_s,
        parent: s["parent"],
        description: (s["description"] || "").to_s,
        cpus: s["cpus"].to_i,
        mem: s["mem"].to_i,
        ports: (s["ports"] || []).map { |p| PortForward.from_row(p) },
        size: s["size"],
        created_at: s["createdAt"].to_i
      )
    end

    # Map a +bsdkrun snapshots --json+ row (snake_case).
    # @param row [Hash]
    # @return [SnapshotInfo]
    def self.from_row(row)
      new(
        id: row["id"].to_s,
        name: row["name"].to_s,
        machine_id: row["machine_id"].to_s,
        machine_name: (row["machine_name"] || "").to_s,
        kind: row["kind"].to_s,
        image: (row["image"] || "").to_s,
        path: (row["path"] || "").to_s,
        parent: row["parent"],
        description: (row["description"] || "").to_s,
        cpus: row["cpus"].to_i,
        mem: row["mem"].to_i,
        ports: (row["ports"] || []).map { |p| PortForward.from_row(p) },
        size: row["size"],
        created_at: row["created_at"].to_i
      )
    end
  end

  # The Docker engine VM: whether it is up, and how to reach it.
  #
  # bsdkrun runs one +docker:dind+ microVM and serves its API on a host unix
  # socket, so the host's own +docker+ CLI drives the same engine.
  DockerStatus = Data.define(
    :running, :machine_id, :machine_running, :socket, :socket_ready, :api_port,
    :version, :containers, :images, :mounts, :disk, :disk_size
  ) do
    # @param s [Hash] a GraphQL +DockerStatus+ (camelCase).
    # @return [DockerStatus]
    def self.from_graphql(s)
      new(
        running: !!s["running"],
        machine_id: s["machineId"],
        machine_running: !!s["machineRunning"],
        socket: s["socket"].to_s,
        socket_ready: !!s["socketReady"],
        api_port: to_i_or_nil(s["apiPort"]),
        version: s["version"],
        containers: to_i_or_nil(s["containers"]),
        images: to_i_or_nil(s["images"]),
        mounts: Array(s["mounts"]),
        disk: s["disk"],
        disk_size: to_i_or_nil(s["diskSize"])
      )
    end

    # @param row [Hash] a +bsdkrun docker status --json+ row (snake_case).
    # @return [DockerStatus]
    def self.from_row(row)
      new(
        running: !!row["running"],
        machine_id: row["machine_id"],
        machine_running: !!row["machine_running"],
        socket: row["socket"].to_s,
        socket_ready: !!row["socket_ready"],
        api_port: to_i_or_nil(row["api_port"]),
        version: row["version"],
        containers: to_i_or_nil(row["containers"]),
        images: to_i_or_nil(row["images"]),
        mounts: Array(row["mounts"]),
        disk: row["disk"],
        disk_size: to_i_or_nil(row["disk_size"])
      )
    end
  end

  # A container in the Docker engine VM — a trimmed +docker ps+ row.
  DockerContainer = Data.define(
    :id, :name, :image, :command, :state, :status, :ports, :created
  ) do
    # @return [Boolean] whether the container is up.
    def running?
      state == "running"
    end

    # Both the GraphQL object and +docker ps --json+ use these field names.
    # @param c [Hash]
    # @return [DockerContainer]
    def self.from_graphql(c)
      new(
        id: c["id"].to_s,
        name: c["name"].to_s,
        image: c["image"].to_s,
        command: (c["command"] || "").to_s,
        state: c["state"].to_s,
        status: c["status"].to_s,
        ports: Array(c["ports"]),
        created: c["created"].to_i
      )
    end

    class << self
      alias from_row from_graphql
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

  # The outcome of a {Bsdkrun::Client} lifecycle mutation (+stopMachine+,
  # +startMachine+, +removeMachines+, +updateMachine+, +commitMachine+, ...).
  # A non-zero +exit_code+ is a value to inspect, not necessarily a failure —
  # mirrors +daemon/src/graphql.rs+'s +CommandResult+.
  #
  # @!attribute [r] exit_code
  #   @return [Integer]
  # @!attribute [r] stdout
  #   @return [String]
  # @!attribute [r] stderr
  #   @return [String]
  CommandResult = Data.define(:exit_code, :stdout, :stderr)

  # A {Bsdkrun::Client#exec} result: the guest command's exit status and its
  # combined (stdout+stderr, in arrival order) output, decoded from the
  # +shellOutput+ subscription's base64 frames.
  #
  # @!attribute [r] exit_code
  #   @return [Integer]
  # @!attribute [r] output
  #   @return [String] binary-safe combined output.
  ExecResult = Data.define(:exit_code, :output)

  # An open interactive shell session, as reported by +openShell+ / the
  # +shellSessions+ query. Mirrors +daemon/src/graphql.rs+'s +ShellSessionInfo+.
  #
  # @!attribute [r] id
  #   @return [String]
  # @!attribute [r] machine_id
  #   @return [String]
  # @!attribute [r] finished
  #   @return [Boolean]
  # @!attribute [r] truncated
  #   @return [Boolean] whether buffered output was dropped to stay under the
  #     session buffer cap.
  ShellSessionInfo = Data.define(:id, :machine_id, :finished, :truncated)
end
