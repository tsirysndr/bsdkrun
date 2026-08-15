defmodule Bsdkrun.FileSystem do
  @moduledoc """
  Files in a running sandbox.

  Every call goes through the guest's exec agent, so the sandbox has to be
  running — there is no offline write. Each function takes the machine id, in
  keeping with the rest of the SDK's functional shape.

      Bsdkrun.FileSystem.write_file("web", "/app/main.py", "print('hi')")
      {:ok, text} = Bsdkrun.FileSystem.read_file("web", "/app/out.json")
      Bsdkrun.FileSystem.upload("web", "./src", "/app/src")
      Bsdkrun.FileSystem.download("web", "/app/dist", "./dist", recursive: true)
  """

  alias Bsdkrun.{Cli, Error}

  @doc """
  Write `data` to `path` in the guest, creating parent directories.

  Returns `:ok` or `{:error, %Bsdkrun.Error{kind: :file_transfer}}`.
  """
  @spec write_file(String.t(), String.t(), iodata()) :: :ok | {:error, Error.t()}
  def write_file(id, path, data) do
    ["cp", "-", "#{id}:#{path}"]
    |> Cli.run(stdin: data)
    |> check(path)
    |> case do
      {:ok, _} -> :ok
      error -> error
    end
  end

  @doc """
  Read `path` from the guest, returning `{:ok, binary}`.

  The result is a raw binary, so it is equally correct for text and for a PNG.
  """
  @spec read_file(String.t(), String.t()) :: {:ok, binary()} | {:error, Error.t()}
  def read_file(id, path) do
    ["cp", "#{id}:#{path}", "-"]
    |> Cli.run()
    |> check(path)
    |> case do
      {:ok, res} -> {:ok, res.stdout}
      error -> error
    end
  end

  @doc """
  Copy a host file or directory into the guest.

  A directory's *contents* land in `remote_path`, so
  `upload(id, "./src", "/app/src")` leaves the guest's `/app/src` holding what
  `./src` holds. Whether it recurses is decided by looking at the local path,
  so callers do not have to say which kind of thing they are copying.
  """
  @spec upload(String.t(), Path.t(), String.t()) :: :ok | {:error, Error.t()}
  def upload(id, local_path, remote_path) do
    if File.exists?(local_path) do
      recursive = if File.dir?(local_path), do: ["-r"], else: []

      (["cp"] ++ recursive ++ [to_string(local_path), "#{id}:#{remote_path}"])
      |> Cli.run()
      |> check(to_string(local_path))
      |> case do
        {:ok, _} -> :ok
        error -> error
      end
    else
      {:error,
       Error.file_transfer(
         "cannot upload #{local_path}: no such file or directory",
         to_string(local_path)
       )}
    end
  end

  @doc """
  Copy a file or directory out of the guest onto the host.

  Pass `recursive: true` for a directory; unlike `upload/3` it cannot be
  detected here, because the path lives in the guest and answering would cost
  an extra round trip on every call.
  """
  @spec download(String.t(), String.t(), Path.t(), keyword()) :: :ok | {:error, Error.t()}
  def download(id, remote_path, local_path, opts \\ []) do
    recursive = if Keyword.get(opts, :recursive, false), do: ["-r"], else: []

    (["cp"] ++ recursive ++ ["#{id}:#{remote_path}", to_string(local_path)])
    |> Cli.run()
    |> check(remote_path)
    |> case do
      {:ok, _} -> :ok
      error -> error
    end
  end

  # The CLI already explains these well; strip its "Error: " prefix.
  defp check(%{exit_code: 0} = res, _path), do: {:ok, res}

  defp check(res, path) do
    text =
      res.stderr
      |> to_string()
      |> String.trim()
      |> String.replace_prefix("Error: ", "")

    message = if text == "", do: "file transfer failed for #{path}", else: text
    {:error, Error.file_transfer(message, path)}
  end
end
