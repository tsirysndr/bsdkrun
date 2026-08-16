package bsdkrun

import java.io.ByteArrayOutputStream
import java.nio.charset.StandardCharsets.UTF_8
import scala.jdk.CollectionConverters.*

/** Runs the `bsdkrun` CLI and captures its output.
  *
  * Every invocation is prefixed with the global `--log-level` flag (default 0)
  * so the SDK's captured output stays clean. Raise it for boot diagnostics.
  */
object Proc:

  /** The buffered result of a `bsdkrun` invocation. */
  final case class RawResult(stdout: String, stderr: String, exitCode: Int)

  /** Like [[RawResult]], but stdout is left as bytes.
    *
    * Decoding stdout as UTF-8 replaces every invalid byte — fine for JSON,
    * ruinous for `cp ID:path -` reading a PNG. Only stderr is decoded here,
    * because it is always a message.
    */
  final case class BinaryResult(stdout: Array[Byte], stderr: String, exitCode: Int)

  /** Options for one invocation. */
  final case class Options(
      env: Map[String, String] = Map.empty,
      stdin: Option[Array[Byte]] = None,
      logLevel: Int = 0,
      onStdout: Option[Array[Byte] => Unit] = None,
      onStderr: Option[Array[Byte] => Unit] = None
  )

  private def withGlobals(args: Seq[String], logLevel: Int): Seq[String] =
    Seq("--log-level", logLevel.toString) ++ args

  /** Run `bsdkrun <args>` and return `(stdout, stderr, exit code)` as bytes —
    * the undecoded form both [[run]] and [[runBinary]] are built on.
    */
  private def runRaw(
      args: Seq[String],
      opts: Options
  ): Either[BsdkrunError, (Array[Byte], Array[Byte], Int)] =
    Binary.resolve().flatMap: binary =>
      try
        val builder = new java.lang.ProcessBuilder((binary +: withGlobals(args, opts.logLevel)).asJava)
        opts.env.foreach((k, v) => builder.environment().nn.put(k, v))
        val proc = builder.start().nn

        // Feed stdin and drain both streams concurrently. A child that starts
        // producing output before it has consumed all of stdin would otherwise
        // deadlock against a sequential write — the same job Python's
        // `communicate()` does with its worker threads.
        val stdinThread = new Thread(() =>
          val os = proc.getOutputStream.nn
          try opts.stdin.foreach(os.write)
          catch case _: java.io.IOException => () // the child exited early
          finally os.close()
        )
        val stdout = new ByteArrayOutputStream()
        val stderr = new ByteArrayOutputStream()
        val outThread = drain(proc.getInputStream.nn, stdout, opts.onStdout)
        val errThread = drain(proc.getErrorStream.nn, stderr, opts.onStderr)

        stdinThread.start()
        outThread.start()
        errThread.start()
        val code = proc.waitFor()
        stdinThread.join()
        outThread.join()
        errThread.join()

        Right((stdout.toByteArray.nn, stderr.toByteArray.nn, code))
      catch
        case e: java.io.IOException =>
          Left(BsdkrunError.CommandFailed(-1, "", s"could not run $binary: ${e.getMessage}", args.mkString(" ")))

  private def drain(
      in: java.io.InputStream,
      into: ByteArrayOutputStream,
      callback: Option[Array[Byte] => Unit]
  ): Thread =
    new Thread(() =>
      val buf = new Array[Byte](8192)
      var n = in.read(buf)
      while n > 0 do
        into.write(buf, 0, n)
        callback.foreach(cb => cb(buf.take(n)))
        n = in.read(buf)
    )

  /** Run `bsdkrun <args>` to completion, buffering stdout/stderr as text. */
  def run(args: Seq[String], opts: Options = Options()): Either[BsdkrunError, RawResult] =
    runRaw(args, opts).map: (out, err, code) =>
      RawResult(new String(out, UTF_8), new String(err, UTF_8), code)

  /** Run `bsdkrun <args>` and keep stdout as raw bytes. */
  def runBinary(args: Seq[String], opts: Options = Options()): Either[BsdkrunError, BinaryResult] =
    runRaw(args, opts).map: (out, err, code) =>
      BinaryResult(out, new String(err, UTF_8), code)

  /** Like [[run]], but a non-zero exit becomes [[BsdkrunError.CommandFailed]]
    * tagged with `label`.
    */
  def runChecked(
      args: Seq[String],
      label: String,
      opts: Options = Options()
  ): Either[BsdkrunError, RawResult] =
    run(args, opts).flatMap: res =>
      if res.exitCode == 0 then Right(res)
      else Left(BsdkrunError.CommandFailed(res.exitCode, res.stdout, res.stderr, label))

  /** Run `bsdkrun <args>` inheriting this process's stdio, for the interactive
    * subcommands. Blocks until the child exits and returns its exit code.
    */
  def spawn(args: Seq[String], logLevel: Int = 0): Either[BsdkrunError, Int] =
    Binary.resolve().map: binary =>
      new java.lang.ProcessBuilder((binary +: withGlobals(args, logLevel)).asJava)
        .inheritIO()
        .nn
        .start()
        .nn
        .waitFor()
