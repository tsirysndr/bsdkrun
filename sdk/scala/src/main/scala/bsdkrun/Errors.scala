package bsdkrun

/** Every failure the SDK reports.
  *
  * A sealed hierarchy rather than exceptions: calls return
  * `Either[BsdkrunError, A]`, so a caller composes them in a `for`
  * comprehension and the compiler keeps track of what can still go wrong. The
  * `*OrThrow` variants on each API wrap these in [[BsdkrunException]] for code
  * that would rather not thread an `Either`.
  */
sealed abstract class BsdkrunError(val message: String) extends Product with Serializable:
  override def toString: String = message

object BsdkrunError:

  /** The `bsdkrun` binary could not be located on this host. */
  final case class BinaryNotFound(searched: Seq[String])
      extends BsdkrunError(
        "could not find the \"bsdkrun\" binary. Set BSDKRUN_BIN, add it to PATH, " +
          s"or call Bsdkrun.setBinaryPath(...). Looked in: ${searched.mkString(", ")}"
      )

  /** A `bsdkrun` invocation — or a guest command run through one — exited
    * non-zero. Carries both streams so the caller can report what happened.
    */
  final case class CommandFailed(
      exitCode: Int,
      stdout: String,
      stderr: String,
      command: String
  ) extends BsdkrunError(
        s"command failed (exit $exitCode): $command" +
          (if stderr.trim.nonEmpty then s"\n${stderr.trim}" else "")
      )

  /** No machine matched the given id or prefix. */
  final case class SandboxNotFound(id: String)
      extends BsdkrunError(s"no sandbox found matching id \"$id\"")

  /** A guest filesystem operation was refused. */
  final case class FileTransfer(path: String, detail: String)
      extends BsdkrunError(detail)

  /** Create options were internally inconsistent — a required field left
    * empty, or a guest kind that does not exist.
    */
  final case class InvalidOptions(detail: String) extends BsdkrunError(detail)

  /** `bsdkrun --json` produced output the SDK could not decode. Carries the raw
    * text, because the useful thing is almost always what was actually printed.
    */
  final case class DecodeFailed(label: String, raw: String)
      extends BsdkrunError(s"could not decode $label output: $raw")

  /** A GraphQL- or transport-level failure talking to a remote `bsdkrund`.
    *
    * `code` carries the response's `extensions.code` when the daemon set one
    * (e.g. `"INVALID_ARGUMENT"`); it is `None` for a transport failure or a
    * malformed response.
    */
  final case class GraphQL(detail: String, code: Option[String] = None)
      extends BsdkrunError(detail)

  /** The daemon rejected the bearer token: an HTTP 401, a GraphQL error whose
    * `extensions.code` is `"UNAUTHENTICATED"`, or a subscription socket that
    * closed before `connection_ack` ever arrived.
    */
  final case class Auth(detail: String = "the daemon rejected this token")
      extends BsdkrunError(detail)

  /** A remote client was asked for without a URL to talk to. */
  final case class MissingConfig(detail: String) extends BsdkrunError(detail)

/** Thrown by the `*OrThrow` variants, so a caller who prefers exceptions to
  * `Either` still gets the structured error underneath.
  */
final class BsdkrunException(val error: BsdkrunError)
    extends RuntimeException(error.message)
