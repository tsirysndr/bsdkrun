defmodule Bsdkrun.Cli do
  @moduledoc """
  Shells out to the `bsdkrun` binary via `System.cmd/3`.

  Every invocation is prefixed with the global `--log-level` flag (default `0`)
  so the SDK's captured output stays clean. Because `System.cmd/3` cannot split
  stderr from stdout — nor feed stdin — the child is wrapped in a tiny
  `/bin/sh -c` that redirects `2>` (and optionally `0<`) to temp files, which we
  read back and delete. The result is a map of `stdout`, `stderr`, `exit_code`.
  """

  alias Bsdkrun.{Binary, Error}

  @type result :: %{stdout: String.t(), stderr: String.t(), exit_code: integer()}

  @typedoc "Options accepted by `run/2`."
  @type run_opt ::
          {:log_level, non_neg_integer()}
          | {:env, map() | keyword()}
          | {:stdin, iodata()}

  @doc """
  Run `bsdkrun <args>` to completion, buffering stdout/stderr separately.

  Options:

    * `:log_level` — the global `--log-level` (default `0`).
    * `:env`       — extra environment variables merged onto the process env.
    * `:stdin`     — data piped to the child's stdin.
  """
  @spec run([String.t()], [run_opt()]) :: result()
  def run(args, opts \\ []) when is_list(args) do
    bin = Binary.resolve!()
    log_level = Keyword.get(opts, :log_level, 0)
    full = ["--log-level", to_string(log_level)] ++ args

    extra_env =
      opts
      |> Keyword.get(:env, %{})
      |> Enum.map(fn {k, v} -> {to_string(k), to_string(v)} end)

    stderr_path = tmp_path("stderr")

    {redirs, stdin_env, cleanup} =
      case Keyword.get(opts, :stdin) do
        nil ->
          {[~s(2>"$__BSDKRUN_STDERR")], [], []}

        data ->
          stdin_path = tmp_path("stdin")
          File.write!(stdin_path, data)

          {[~s(0<"$__BSDKRUN_STDIN"), ~s(2>"$__BSDKRUN_STDERR")],
           [{"__BSDKRUN_STDIN", stdin_path}], [stdin_path]}
      end

    script = ~s(exec "$@" ) <> Enum.join(redirs, " ")
    env = [{"__BSDKRUN_STDERR", stderr_path} | stdin_env] ++ extra_env

    try do
      {stdout, code} =
        System.cmd("/bin/sh", ["-c", script, "sh", bin] ++ full,
          env: env,
          stderr_to_stdout: false
        )

      stderr =
        case File.read(stderr_path) do
          {:ok, contents} -> contents
          _ -> ""
        end

      %{stdout: stdout, stderr: stderr, exit_code: code}
    after
      File.rm(stderr_path)
      Enum.each(cleanup, &File.rm/1)
    end
  end

  @doc """
  Run `bsdkrun <args>` and return `{:ok, result}` on exit 0, or
  `{:error, %Bsdkrun.Error{}}` (kind `:command_failed`) otherwise. `label`
  names the command for error messages.
  """
  @spec checked([String.t()], String.t(), [run_opt()]) ::
          {:ok, result()} | {:error, Error.t()}
  def checked(args, label, opts \\ []) do
    res = run(args, opts)

    if res.exit_code == 0 do
      {:ok, res}
    else
      {:error, Error.command_failed(res.exit_code, res.stdout, res.stderr, label)}
    end
  end

  defp tmp_path(label) do
    Path.join(
      System.tmp_dir!(),
      "bsdkrun-#{System.unique_integer([:positive])}-#{label}"
    )
  end
end
