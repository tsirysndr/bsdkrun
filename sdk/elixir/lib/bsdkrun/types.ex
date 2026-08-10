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
            kind: String.t(),
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
            ports: [PortForward.t()]
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
      :ports
    ]
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
      kind: to_string(row["kind"]),
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

  defp truthy(true), do: true
  defp truthy(_), do: false

  defp num(nil), do: nil
  defp num(v) when is_integer(v), do: v
  defp num(v) when is_float(v), do: trunc(v)
  defp num(v) when is_binary(v), do: String.to_integer(v)
end
