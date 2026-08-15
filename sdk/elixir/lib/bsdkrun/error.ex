defmodule Bsdkrun.Error do
  @moduledoc """
  The single exception type raised (or returned in `{:error, _}` tuples) by the
  SDK. `kind` discriminates the failure:

    * `:binary_not_found`  — the `bsdkrun` binary could not be located.
    * `:command_failed`    — a `bsdkrun` invocation exited non-zero. Carries
      `exit_code`, `stdout`, `stderr`, and the `label` of the command.
    * `:sandbox_not_found` — no machine matched the given id / prefix.
    * `:auth_error`        — the remote daemon (`Bsdkrun.Client`) rejected the
      bearer token, either over HTTP (401) or a GraphQL error with
      `extensions.code == "UNAUTHENTICATED"`, or a `graphql-transport-ws`
      socket that closed before `connection_ack` ever arrived.
    * `:graphql_error`     — any other `Bsdkrun.Client` failure: an
      unreachable daemon, a non-GraphQL response, or a GraphQL `errors[]`
      entry that isn't an auth failure. `code` carries the GraphQL
      `extensions.code`, if any.
    * `:config_error`      — invalid or missing `Bsdkrun.Client.from_env/0`
      configuration.
  """

  @typedoc "The category of failure this error represents."
  @type kind ::
          :binary_not_found
          | :command_failed
          | :sandbox_not_found
          | :file_transfer
          | :auth_error
          | :graphql_error
          | :config_error

  @type t :: %__MODULE__{
          kind: kind(),
          message: String.t(),
          exit_code: integer() | nil,
          stdout: String.t() | nil,
          stderr: String.t() | nil,
          label: String.t() | nil,
          searched: [String.t()] | nil,
          code: String.t() | nil
        }

  defexception [
    :kind,
    :message,
    :exit_code,
    :stdout,
    :stderr,
    :label,
    :searched,
    :code
  ]

  @impl true
  def message(%__MODULE__{message: message}), do: message

  @doc "Build a `:binary_not_found` error listing the paths that were searched."
  @spec binary_not_found([String.t()]) :: t()
  def binary_not_found(searched) do
    %__MODULE__{
      kind: :binary_not_found,
      searched: searched,
      message:
        ~s(could not find the "bsdkrun" binary. Set BSDKRUN_BIN, add it to PATH, ) <>
          "or call Bsdkrun.Binary.set_binary_path/1. Looked in: " <>
          Enum.join(searched, ", ")
    }
  end

  @doc "Build a `:command_failed` error from a captured CLI result."
  @spec command_failed(integer(), String.t(), String.t(), String.t()) :: t()
  def command_failed(exit_code, stdout, stderr, label) do
    trimmed = String.trim(stderr || "")
    detail = if trimmed == "", do: "", else: "\n" <> trimmed

    %__MODULE__{
      kind: :command_failed,
      exit_code: exit_code,
      stdout: stdout,
      stderr: stderr,
      label: label,
      message: "command failed (exit #{exit_code}): #{label}" <> detail
    }
  end

  @doc "Build a `:sandbox_not_found` error for the given id / prefix."
  @spec sandbox_not_found(String.t()) :: t()
  def sandbox_not_found(id) do
    %__MODULE__{
      kind: :sandbox_not_found,
      message: "no sandbox found matching id #{inspect(id)}"
    }
  end

  @doc """
  Build a `:file_transfer` error — a guest filesystem operation was refused.

  `label` carries the path that could not be transferred.
  """
  @spec file_transfer(String.t(), String.t()) :: t()
  def file_transfer(message, path) do
    %__MODULE__{kind: :file_transfer, message: message, label: path}
  end

  @doc "Build an `:auth_error` — the daemon rejected the bearer token."
  @spec auth_error(String.t()) :: t()
  def auth_error(message \\ "the daemon rejected this token") do
    %__MODULE__{kind: :auth_error, message: message, code: "UNAUTHENTICATED"}
  end

  @doc """
  Build a `:graphql_error` — an unreachable daemon, a non-GraphQL response, or
  a GraphQL `errors[]` entry that isn't an auth failure. `code` is the
  GraphQL `extensions.code`, if the server sent one.
  """
  @spec graphql_error(String.t(), String.t() | nil) :: t()
  def graphql_error(message, code \\ nil) do
    %__MODULE__{kind: :graphql_error, message: message, code: code}
  end

  @doc "Build a `:config_error` for invalid/missing `Bsdkrun.Client.from_env/0` configuration."
  @spec config_error(String.t()) :: t()
  def config_error(message) do
    %__MODULE__{kind: :config_error, message: message}
  end
end
