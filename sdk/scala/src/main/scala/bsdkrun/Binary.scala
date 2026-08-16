package bsdkrun

import java.io.File
import java.nio.file.{Files, Path, Paths}

/** Locates the `bsdkrun` binary on the host and caches the result.
  *
  * Resolution order (first match wins):
  *
  *   1. an explicit override set via [[setPath]]
  *   1. the `BSDKRUN_BIN` environment variable
  *   1. `bsdkrun` on `PATH`
  *   1. an in-repo dev build: `<repo>/target/release/bsdkrun`, then
  *      `<repo>/target/debug/bsdkrun`
  *
  * [[candidates]] and [[resolveWith]] take an explicit [[Env]] so the discovery
  * logic is unit-testable without mutating real process state — the JVM has no
  * portable way to change its own environment, unlike Ruby's `ENV[...]=`.
  */
object Binary:

  /** The host state discovery reads, captured so tests can supply their own. */
  final case class Env(
      override_ : Option[String],
      bsdkrunBin: Option[String],
      path: Option[String],
      repoRoot: Option[Path]
  )

  @volatile private var overridePath: Option[String] = None
  @volatile private var resolved: Option[String] = None

  /** Force the SDK to use a specific `bsdkrun` binary, bypassing discovery. */
  def setPath(path: String): Unit = synchronized:
    overridePath = Some(path)
    resolved = None

  /** The current explicit override, if any. */
  def overridden: Option[String] = overridePath

  /** Drop cached discovery state and any override. Mainly for tests. */
  def reset(): Unit = synchronized:
    overridePath = None
    resolved = None

  private def nonEmpty(s: String | Null): Option[String] =
    Option(s).map(_.nn).filter(_.nonEmpty)

  /** The real host state, for the no-argument entry points. */
  private def hostEnv: Env = Env(
    override_ = overridePath,
    bsdkrunBin = nonEmpty(System.getenv("BSDKRUN_BIN")),
    path = nonEmpty(System.getenv("PATH")),
    repoRoot = repoRoot
  )

  /** The monorepo root, when this class is running from a source checkout
    * rather than a packaged jar: `sdk/scala/target/...` walks back up to the
    * repo. Mirrors the same dev-build fallback the other SDKs have, which is
    * what makes `sbt test` in a checkout find the freshly built CLI.
    */
  private def repoRoot: Option[Path] =
    val marker = "/sdk/scala/"
    val here = Option(getClass.getProtectionDomain.nn.getCodeSource)
      .flatMap(cs => Option(cs.nn.getLocation))
      .map(_.nn.getPath.nn)
    here.flatMap: p =>
      val idx = p.indexOf(marker)
      if idx >= 0 then Some(Paths.get(p.substring(0, idx)).nn) else None

  private def isExecutableFile(p: Path): Boolean =
    Files.isRegularFile(p) && Files.isExecutable(p)

  /** Cross-platform `PATH` lookup. `pathEnv` is a raw `PATH`-style string. */
  private def which(name: String, pathEnv: Option[String]): Option[String] =
    pathEnv.getOrElse("").split(File.pathSeparator).iterator
      .filter(_.nonEmpty)
      .map(dir => Paths.get(dir, name).nn)
      .find(isExecutableFile)
      .map(_.toString)

  /** Candidate locations, in priority order. A pure function of `env`. */
  def candidates(env: Env): Seq[String] =
    val builder = Seq.newBuilder[String]
    env.override_.foreach(builder += _)
    env.bsdkrunBin.foreach(builder += _)
    which("bsdkrun", env.path).foreach(builder += _)
    env.repoRoot.foreach: root =>
      builder += root.resolve("target/release/bsdkrun").nn.toString
      builder += root.resolve("target/debug/bsdkrun").nn.toString
    builder.result()

  /** Candidate locations for the real host. */
  def candidates(): Seq[String] = candidates(hostEnv)

  /** Resolve the binary against an explicit environment, without caching. */
  def resolveWith(env: Env): Either[BsdkrunError, String] =
    val searched = candidates(env)
    val found = searched.find: candidate =>
      if candidate.contains(File.separator) then Files.exists(Paths.get(candidate).nn)
      else which(candidate, env.path).isDefined
    found.toRight(BsdkrunError.BinaryNotFound(searched))

  /** Resolve (and cache) the path to the `bsdkrun` binary. */
  def resolve(): Either[BsdkrunError, String] =
    resolved match
      case Some(path) => Right(path)
      case None =>
        val out = resolveWith(hostEnv)
        out.foreach(p => synchronized { resolved = Some(p) })
        out
