package bsdkrun

/** The result of running a command inside a guest. */
final case class CommandResult(
    stdout: String,
    stderr: String,
    exitCode: Int,
    command: String
):
  def ok: Boolean = exitCode == 0

  /** stdout with trailing whitespace trimmed — what you almost always want. */
  def text: String = stdout.trim

  /** stdout split into lines, blank ones dropped. */
  def lines: Seq[String] = text.linesIterator.filter(_.nonEmpty).toSeq

  /** Turn a non-zero exit into a `Left`, so a `for` comprehension short-circuits. */
  def checked: Either[BsdkrunError, CommandResult] =
    if ok then Right(this)
    else Left(BsdkrunError.CommandFailed(exitCode, stdout, stderr, command))

/** Quoting for the `sh` interpolator.
  *
  * Interpolated values are single-quoted so a value containing `;` or `$` is
  * data, not syntax — the same guarantee the other SDKs' `sh` template gives.
  * [[Shell.raw]] opts a fragment out when the caller really does mean shell
  * source.
  */
object Shell:

  /** Wraps a fragment that should be spliced into a script unquoted. */
  final case class Raw(value: String)

  /** Mark a fragment as literal shell source, bypassing quoting. */
  def raw(value: String): Raw = Raw(value)

  /** POSIX single-quoting: wrap in `'`, and close/escape/reopen for each `'`. */
  def quote(value: String): String =
    "'" + value.replace("'", "'\\''") + "'"

  private[bsdkrun] def render(value: Any): String = value match
    case Raw(v)          => v
    case s: String       => quote(s)
    case xs: Iterable[?] => xs.map(render).mkString(" ")
    case other           => quote(String.valueOf(other))

  /** The `sh"..."` interpolator: builds a script with every interpolation
    * quoted.
    *
    * {{{
    * val script = sh"echo \$greeting > \${path}"
    * }}}
    */
  extension (sc: StringContext)
    def sh(args: Any*): String =
      val parts = sc.parts.iterator
      val values = args.iterator
      val out = new StringBuilder(parts.next())
      while values.hasNext do
        out.append(render(values.next()))
        out.append(parts.next())
      out.toString
