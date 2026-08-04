defmodule Bsdkrun.Error do
  @moduledoc """
  The single exception type raised (or returned in `{:error, _}` tuples) by the
  SDK. `kind` discriminates the failure:

    * `:binary_not_found` — the `bsdkrun` binary could not be located.
    * `:command_failed`   — a `bsdkrun` invocation exited non-zero. Carries
      `exit_code`, `stdout`, `stderr`, and the `label` of the command.
    * `:sandbox_not_found` — no machine matched the given id / prefix.
  """

  @typedoc "The category of failure this error represents."
  @type kind :: :binary_not_found | :command_failed | :sandbox_not_found

  @type t :: %__MODULE__{
          kind: kind(),
          message: String.t(),
          exit_code: integer() | nil,
          stdout: String.t() | nil,
          stderr: String.t() | nil,
          label: String.t() | nil,
          searched: [String.t()] | nil
        }

  defexception [
    :kind,
    :message,
    :exit_code,
    :stdout,
    :stderr,
    :label,
    :searched
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
end
