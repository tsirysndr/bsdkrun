defmodule Bsdkrun.Types do
  @moduledoc """
  Typed structs mirroring `bsdkrun`'s JSON output, plus mapping helpers that
  turn a decoded `--json` row (string keys) into the corresponding struct.
  """

  defmodule PortForward do
    @moduledoc "A host->guest TCP port forward, as reported by `bsdkrun ps --json`."

    @type t :: %__MODULE__{
            bind: String.t(),
            host: integer(),
            guest: integer()
          }

    defstruct [:bind, :host, :guest]
  end

  defmodule SandboxInfo do
    @moduledoc "A machine as reported by `bsdkrun ps --json`."

    @type t :: %__MODULE__{
            id: String.t(),
            name: String.t() | nil,
            image: String.t(),
            kind: Bsdkrun.Args.os(),
            command: String.t(),
            status: String.t(),
            running: boolean(),
            exit_code: integer() | nil,
            pid: integer() | nil,
            detached: boolean(),
            cpus: integer(),
            mem: integer(),
            volume: String.t() | nil,
            state_dir: String.t(),
            network: String.t() | nil,
            net_ip: String.t() | nil,
            created_at: integer(),
            finished_at: integer() | nil,
            ports: [PortForward.t()],
            origin: String.t() | nil
          }

    defstruct [
      :id,
      :name,
      :image,
      :kind,
      :command,
      :status,
      :running,
      :exit_code,
      :pid,
      :detached,
      :cpus,
      :mem,
      :volume,
      :state_dir,
      :network,
      :net_ip,
      :created_at,
      :finished_at,
      :ports,
      :origin
    ]
  end

  defmodule SnapshotInfo do
    @moduledoc """
    A machine snapshot: one machine's disk state, captured under a name.

    A copy-on-write clone rather than a memory image — the files the guest
    wrote, not what it was executing. `Bsdkrun.Client.branch/3` boots a new
    machine from one; `Bsdkrun.Client.restore/4` puts one back.
    """

    @type t :: %__MODULE__{
            id: String.t(),
            name: String.t(),
            machine_id: String.t(),
            machine_name: String.t(),
            kind: String.t(),
            image: String.t(),
            path: String.t(),
            parent: String.t() | nil,
            description: String.t(),
            cpus: integer(),
            mem: integer(),
            ports: [PortForward.t()],
            size: String.t() | nil,
            created_at: integer()
          }

    defstruct [
      :id,
      :name,
      :machine_id,
      :machine_name,
      :kind,
      :image,
      :path,
      :parent,
      :description,
      :cpus,
      :mem,
      :ports,
      :size,
      :created_at
    ]
  end

  defmodule AiAgent do
    @moduledoc """
    A coding agent bsdkrun can sandbox.

    Each runs in a disposable microVM with a persistent login, a shared skills
    store, and only the folder you choose to share.
    """

    @type t :: %__MODULE__{
            id: String.t(),
            label: String.t(),
            flavor: String.t(),
            description: String.t(),
            installed: boolean(),
            running: integer()
          }

    defstruct [:id, :label, :flavor, :description, :installed, :running]
  end

  defmodule AiSession do
    @moduledoc "One agent sandbox. It is a machine, so `logs`/`stop` work on `id`."

    @type t :: %__MODULE__{
            id: String.t(),
            name: String.t(),
            agent: String.t(),
            running: boolean(),
            workspace: String.t() | nil,
            created_at: integer()
          }

    defstruct [:id, :name, :agent, :running, :workspace, :created_at]
  end

  defmodule DockerStatus do
    @moduledoc """
    The Docker engine VM: whether it is up, and how to reach it.

    bsdkrun runs one `docker:dind` microVM and serves its API on a host unix
    socket, so the host's own `docker` CLI drives the same engine.
    """

    @type t :: %__MODULE__{
            running: boolean(),
            machine_id: String.t() | nil,
            machine_running: boolean(),
            socket: String.t(),
            socket_ready: boolean(),
            api_port: integer() | nil,
            version: String.t() | nil,
            containers: integer() | nil,
            images: integer() | nil,
            mounts: [String.t()],
            disk: String.t() | nil,
            disk_size: integer() | nil
          }

    defstruct [
      :running,
      :machine_id,
      :machine_running,
      :socket,
      :socket_ready,
      :api_port,
      :version,
      :containers,
      :images,
      :mounts,
      :disk,
      :disk_size
    ]
  end

  defmodule DockerContainer do
    @moduledoc "A container in the Docker engine VM — a trimmed `docker ps` row."

    @type t :: %__MODULE__{
            id: String.t(),
            name: String.t(),
            image: String.t(),
            command: String.t(),
            state: String.t(),
            status: String.t(),
            ports: [String.t()],
            created: integer()
          }

    defstruct [:id, :name, :image, :command, :state, :status, :ports, :created]

    @doc "Whether the container is up."
    @spec running?(t()) :: boolean()
    def running?(%__MODULE__{state: state}), do: state == "running"
  end

  defmodule ImageInfo do
    @moduledoc "An image as reported by `bsdkrun images --json`."

    @type t :: %__MODULE__{
            id: String.t(),
            reference: String.t(),
            digest: String.t(),
            size: integer(),
            rootfs: String.t(),
            created_at: integer()
          }

    defstruct [:id, :reference, :digest, :size, :rootfs, :created_at]
  end

  defmodule VolumeInfo do
    @moduledoc "A volume as reported by `bsdkrun volume ls --json`."

    @type t :: %__MODULE__{
            name: String.t(),
            guest: String.t() | nil,
            base: String.t() | nil,
            path: String.t(),
            size: String.t(),
            created_at: integer() | nil,
            tracked: boolean()
          }

    defstruct [:name, :guest, :base, :path, :size, :created_at, :tracked]
  end

  defmodule NetworkInfo do
    @moduledoc "A global network as reported by `bsdkrun network ls --json`."

    @type t :: %__MODULE__{
            name: String.t(),
            subnet: String.t(),
            gateway: String.t(),
            members: integer(),
            running: integer(),
            up: boolean(),
            created_at: integer() | nil
          }

    defstruct [:name, :subnet, :gateway, :members, :running, :up, :created_at]
  end

  defmodule CommandResult do
    @moduledoc """
    The outcome of a daemon command run to completion, as reported by
    `Bsdkrun.Client` over GraphQL (`stopMachine`, `removeMachines`, etc.). A
    non-zero `exit_code` is a state to inspect, not necessarily a transport
    failure — mirrors the GraphQL schema's `CommandResult`.
    """

    @type t :: %__MODULE__{
            exit_code: integer(),
            stdout: String.t(),
            stderr: String.t()
          }

    defstruct [:exit_code, :stdout, :stderr]
  end

  defmodule ShellSessionInfo do
    @moduledoc "A shell/exec session opened via the GraphQL `openShell` mutation."

    @type t :: %__MODULE__{
            id: String.t(),
            machine_id: String.t(),
            finished: boolean(),
            truncated: boolean()
          }

    defstruct [:id, :machine_id, :finished, :truncated]
  end

  defmodule Result do
    @moduledoc """
    The captured result of running a command in the guest (`Bsdkrun.Sandbox.exec/3`).
    """

    @type t :: %__MODULE__{
            stdout: String.t(),
            stderr: String.t(),
            exit_code: integer(),
            command: String.t()
          }

    defstruct [:stdout, :stderr, :exit_code, :command]

    @doc "Whether the command succeeded (exit 0)."
    @spec ok?(t()) :: boolean()
    def ok?(%__MODULE__{exit_code: code}), do: code == 0

    @doc "stdout with trailing newlines trimmed — the common case."
    @spec text(t()) :: String.t()
    def text(%__MODULE__{stdout: stdout}), do: String.replace(stdout, ~r/\n+$/, "")

    @doc "Parse stdout as JSON."
    @spec json(t()) :: term()
    def json(%__MODULE__{stdout: stdout}), do: Jason.decode!(stdout)

    @doc "Non-empty stdout lines."
    @spec lines(t()) :: [String.t()]
    def lines(%__MODULE__{stdout: stdout}) do
      stdout |> String.split("\n") |> Enum.reject(&(&1 == ""))
    end
  end

  # --- mapping helpers --------------------------------------------------------

  @doc "Map a `ps --json` row to a `SandboxInfo`."
  @spec sandbox_info(map()) :: SandboxInfo.t()
  def sandbox_info(row) do
    running = truthy(row["running"])

    %SandboxInfo{
      id: to_string(row["id"]),
      name: row["name"],
      image: to_string(row["image"]),
      kind: kind_atom(row["kind"]),
      command: to_string(row["command"] || ""),
      status: if(running, do: "running", else: "exited"),
      running: running,
      exit_code: num(row["exit_code"]),
      pid: num(row["pid"]),
      detached: truthy(row["detached"]),
      cpus: num(row["cpus"]),
      mem: num(row["mem"]),
      volume: row["volume"],
      state_dir: to_string(row["state_dir"]),
      network: row["network"],
      net_ip: row["net_ip"],
      created_at: num(row["created_at"]),
      finished_at: num(row["finished_at"]),
      ports: (row["ports"] || []) |> Enum.map(&port_forward/1)
    }
  end

  @doc "Map a `ports` row (from a `ps --json` row) to a `PortForward`."
  @spec port_forward(map()) :: PortForward.t()
  def port_forward(row) do
    %PortForward{
      bind: row["bind"],
      host: num(row["host"]),
      guest: num(row["guest"])
    }
  end

  @doc """
  Map a GraphQL `Machine` row (camelCase keys, as selected by
  `Bsdkrun.Client`'s `MACHINE_FIELDS`) to a `SandboxInfo` — the same struct
  `sandbox_info/1` builds from the CLI's `ps --json`, just sourced from the
  daemon instead.
  """
  @spec sandbox_info_from_graphql(map()) :: SandboxInfo.t()
  def sandbox_info_from_graphql(row) do
    %SandboxInfo{
      id: to_string(row["id"]),
      name: row["name"],
      image: to_string(row["image"]),
      kind: kind_atom(row["kind"]),
      command: to_string(row["command"] || ""),
      status: to_string(row["status"]),
      running: truthy(row["running"]),
      exit_code: num(row["exitCode"]),
      pid: num(row["pid"]),
      detached: truthy(row["detached"]),
      cpus: num(row["cpus"]),
      mem: num(row["mem"]),
      volume: row["volume"],
      state_dir: to_string(row["stateDir"] || ""),
      network: row["network"],
      net_ip: row["netIp"],
      created_at: num(row["createdAt"]),
      finished_at: num(row["finishedAt"]),
      ports: (row["ports"] || []) |> Enum.map(&port_forward/1),
      origin: row["origin"]
    }
  end

  @doc "Map a GraphQL `Snapshot` row to a `SnapshotInfo` struct."
  @spec snapshot_info_from_graphql(map()) :: SnapshotInfo.t()
  def snapshot_info_from_graphql(row) do
    %SnapshotInfo{
      id: to_string(row["id"]),
      name: to_string(row["name"]),
      machine_id: to_string(row["machineId"] || ""),
      machine_name: to_string(row["machineName"] || ""),
      kind: to_string(row["kind"] || ""),
      image: to_string(row["image"] || ""),
      path: to_string(row["path"] || ""),
      parent: row["parent"],
      description: to_string(row["description"] || ""),
      cpus: num(row["cpus"]),
      mem: num(row["mem"]),
      ports: (row["ports"] || []) |> Enum.map(&port_forward/1),
      size: row["size"],
      created_at: num(row["createdAt"])
    }
  end

  @doc "Map a GraphQL `AiAgent` row to an `AiAgent` struct."
  @spec ai_agent_from_graphql(map()) :: AiAgent.t()
  def ai_agent_from_graphql(row) do
    %AiAgent{
      id: to_string(row["id"] || ""),
      label: to_string(row["label"] || ""),
      flavor: to_string(row["flavor"] || ""),
      description: to_string(row["description"] || ""),
      installed: truthy(row["installed"]),
      running: num(row["running"]) || 0
    }
  end

  @doc "Map a GraphQL `AiSession` row to an `AiSession` struct."
  @spec ai_session_from_graphql(map()) :: AiSession.t()
  def ai_session_from_graphql(row) do
    %AiSession{
      id: to_string(row["id"] || ""),
      name: to_string(row["name"] || ""),
      agent: to_string(row["agent"] || ""),
      running: truthy(row["running"]),
      workspace: row["workspace"],
      created_at: num(row["createdAt"]) || 0
    }
  end

  @doc "Map a GraphQL `DockerStatus` row to a `DockerStatus` struct."
  @spec docker_status_from_graphql(map()) :: DockerStatus.t()
  def docker_status_from_graphql(row) do
    %DockerStatus{
      running: truthy(row["running"]),
      machine_id: row["machineId"],
      machine_running: truthy(row["machineRunning"]),
      socket: to_string(row["socket"] || ""),
      socket_ready: truthy(row["socketReady"]),
      api_port: num(row["apiPort"]),
      version: row["version"],
      containers: num(row["containers"]),
      images: num(row["images"]),
      mounts: row["mounts"] || [],
      disk: row["disk"],
      disk_size: num(row["diskSize"])
    }
  end

  @doc "Map a GraphQL `DockerContainer` row to a `DockerContainer` struct."
  @spec docker_container_from_graphql(map()) :: DockerContainer.t()
  def docker_container_from_graphql(row) do
    %DockerContainer{
      id: to_string(row["id"] || ""),
      name: to_string(row["name"] || ""),
      image: to_string(row["image"] || ""),
      command: to_string(row["command"] || ""),
      state: to_string(row["state"] || ""),
      status: to_string(row["status"] || ""),
      ports: row["ports"] || [],
      created: num(row["created"])
    }
  end

  @doc "Map a GraphQL `CommandResult` row to a `CommandResult` struct."
  @spec command_result_from_graphql(map()) :: CommandResult.t()
  def command_result_from_graphql(row) do
    %CommandResult{
      exit_code: num(row["exitCode"]),
      stdout: to_string(row["stdout"] || ""),
      stderr: to_string(row["stderr"] || "")
    }
  end

  @doc "Map a GraphQL `ShellSessionInfo` row to a `ShellSessionInfo` struct."
  @spec shell_session_info_from_graphql(map()) :: ShellSessionInfo.t()
  def shell_session_info_from_graphql(row) do
    %ShellSessionInfo{
      id: to_string(row["id"]),
      machine_id: to_string(row["machineId"]),
      finished: truthy(row["finished"]),
      truncated: truthy(row["truncated"])
    }
  end

  @doc "Map an `images --json` row to an `ImageInfo`."
  @spec image_info(map()) :: ImageInfo.t()
  def image_info(row) do
    %ImageInfo{
      id: to_string(row["id"]),
      reference: to_string(row["reference"]),
      digest: to_string(row["digest"]),
      size: num(row["size"]),
      rootfs: to_string(row["rootfs"]),
      created_at: num(row["created_at"])
    }
  end

  @doc "Map a `volume ls --json` row to a `VolumeInfo`."
  @spec volume_info(map()) :: VolumeInfo.t()
  def volume_info(row) do
    %VolumeInfo{
      name: to_string(row["name"]),
      guest: row["guest"],
      base: row["base"],
      path: to_string(row["path"]),
      size: to_string(row["size"]),
      created_at: num(row["created_at"]),
      tracked: truthy(row["tracked"])
    }
  end

  @doc "Map a `network ls --json` row to a `NetworkInfo`."
  @spec network_info(map()) :: NetworkInfo.t()
  def network_info(row) do
    %NetworkInfo{
      name: to_string(row["name"]),
      subnet: to_string(row["subnet"]),
      gateway: to_string(row["gateway"]),
      members: num(row["members"]) || 0,
      running: num(row["running"]) || 0,
      up: truthy(row["up"]),
      created_at: num(row["created_at"])
    }
  end

  # `kind` is one of bsdkrun's own guest kinds (`linux` / `freebsd` / `netbsd` /
  # `firmware` / `kernel` / `unikraft` / `solo5` / `nanos` / `osv`) — a small, trusted
  # vocabulary, not user input — so turning it into an atom (matching the
  # `:os` atoms `create/1` already takes) can't exhaust the atom table.
  defp kind_atom(nil), do: nil
  defp kind_atom(kind), do: String.to_atom(to_string(kind))

  defp truthy(true), do: true
  defp truthy(_), do: false

  defp num(nil), do: nil
  defp num(v) when is_integer(v), do: v
  defp num(v) when is_float(v), do: trunc(v)
  defp num(v) when is_binary(v), do: String.to_integer(v)
end
