package bsdkrun

import java.nio.charset.StandardCharsets.UTF_8
import java.nio.file.{Files, Paths}

/** Files in a running sandbox, reached as [[Sandbox.fs]].
  *
  * Every call goes through the guest's exec agent, so the sandbox has to be
  * running — there is no offline write.
  *
  * {{{
  * sbx.fs.writeFile("/app/main.py", "print('hi')")
  * sbx.fs.readText("/app/out.json")
  * sbx.fs.upload("./src", "/app/src")
  * sbx.fs.download("/app/dist", "./dist", recursive = true)
  * }}}
  */
final class FileSystem private[bsdkrun] (id: String):

  /** Write `data` to `path` in the guest, creating parent directories. */
  def writeFile(path: String, data: Array[Byte]): Either[BsdkrunError, Unit] =
    Proc
      .run(Seq("cp", "-", s"$id:$path"), Proc.Options(stdin = Some(data)))
      .flatMap(checked(_, path))

  /** Write text to `path` in the guest, encoded as UTF-8. */
  def writeFile(path: String, text: String): Either[BsdkrunError, Unit] =
    writeFile(path, text.getBytes(UTF_8).nn)

  /** Read `path` from the guest as bytes.
    *
    * Bytes, not a `String`: decoding here would replace every invalid byte, so
    * a PNG read out of a guest would come back silently mangled.
    */
  def readFile(path: String): Either[BsdkrunError, Array[Byte]] =
    Proc.runBinary(Seq("cp", s"$id:$path", "-")).flatMap: res =>
      if res.exitCode == 0 then Right(res.stdout)
      else Left(BsdkrunError.FileTransfer(path, message(res.stderr, path)))

  /** Read `path` from the guest and decode it as UTF-8. */
  def readText(path: String): Either[BsdkrunError, String] =
    readFile(path).map(bytes => new String(bytes, UTF_8))

  /** Copy a host file or directory into the guest.
    *
    * A directory's *contents* land in `remotePath`, so
    * `upload("./src", "/app/src")` leaves the guest's `/app/src` holding what
    * `./src` holds. Whether it recurses is decided by looking at the local
    * path, so callers do not have to say which kind of thing they are copying.
    */
  def upload(localPath: String, remotePath: String): Either[BsdkrunError, Unit] =
    val local = Paths.get(localPath).nn
    if !Files.exists(local) then
      Left(BsdkrunError.FileTransfer(localPath, s"cannot upload $localPath: no such file or directory"))
    else
      val args = Seq("cp") ++
        (if Files.isDirectory(local) then Seq("-r") else Nil) ++
        Seq(localPath, s"$id:$remotePath")
      Proc.run(args).flatMap(checked(_, localPath))

  /** Copy a file or directory out of the guest onto the host.
    *
    * Pass `recursive` for a directory; unlike [[upload]] it cannot be detected
    * here, because the path lives in the guest and answering would cost an
    * extra round trip on every call.
    */
  def download(
      remotePath: String,
      localPath: String,
      recursive: Boolean = false
  ): Either[BsdkrunError, Unit] =
    val args = Seq("cp") ++
      (if recursive then Seq("-r") else Nil) ++
      Seq(s"$id:$remotePath", localPath)
    Proc.run(args).flatMap(checked(_, remotePath))

  private def checked(res: Proc.RawResult, path: String): Either[BsdkrunError, Unit] =
    if res.exitCode == 0 then Right(())
    else Left(BsdkrunError.FileTransfer(path, message(res.stderr, path)))

  /** The CLI already explains these well; strip its `Error: ` prefix. */
  private def message(stderr: String, path: String): String =
    val text = stderr.trim.stripPrefix("Error:").trim
    if text.isEmpty then s"file transfer failed for $path" else text
