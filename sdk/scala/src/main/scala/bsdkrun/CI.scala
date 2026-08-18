package bsdkrun

import java.nio.file.{Files, Path}
import scala.collection.immutable.SortedMap

/** CI workflows defined in code instead of YAML.
  *
  * The builder produces exactly the file `bsdkrun ci` (and tangled's spindle)
  * consumes — [[CIWorkflow.yaml]] is that file, [[CIWorkflow.save]] commits it
  * to `.tangled/workflows/`, and [[CIWorkflow.run]] executes it in a microVM
  * without a file ever touching the repository:
  *
  * {{{
  * CI.workflow("test")
  *   .onPush("main")
  *   .deps("scala", "sbt")
  *   .env("SBT_OPTS", "-Xmx1g")
  *   .step("compile", "sbt compile")
  *   .step("test", "sbt test")
  *   .run()
  * }}}
  *
  * Code is the source of truth and YAML the wire format, in that order —
  * which is why `save` writes a generated-file header: a hand-edit there will
  * be overwritten by the next save.
  */
object CI:
  /** Start a CI workflow definition. */
  def workflow(name: String): CIWorkflow = CIWorkflow(name = name)

final case class CIWorkflow(
    name: String,
    engine: String = "nixery",
    when: Vector[(Vector[String], Vector[String])] = Vector.empty,
    // Sorted for deterministic output: the emitted YAML is committed and
    // diffed, so its ordering must not depend on insertion order.
    dependencies: SortedMap[String, Vector[String]] = SortedMap.empty,
    environment: SortedMap[String, String] = SortedMap.empty,
    steps: Vector[CIStep] = Vector.empty,
    cloneDepth: Option[Int] = None,
    cloneSkip: Boolean = false
):
  /** Override the engine (`nixery` by default). */
  def withEngine(e: String): CIWorkflow = copy(engine = e)

  /** Add a push trigger for the given branches. */
  def onPush(branches: String*): CIWorkflow =
    copy(when = when :+ (Vector("push"), branches.toVector))

  /** Add a pull_request trigger targeting the given branches. */
  def onPullRequest(branches: String*): CIWorkflow =
    copy(when = when :+ (Vector("pull_request"), branches.toVector))

  /** Add nixpkgs dependencies — the toolchain the steps run against. */
  def deps(packages: String*): CIWorkflow = depsFrom("nixpkgs", packages*)

  /** Add dependencies from a custom registry (a flake reference). */
  def depsFrom(registry: String, packages: String*): CIWorkflow =
    copy(dependencies = dependencies.updated(
      registry,
      dependencies.getOrElse(registry, Vector.empty) ++ packages
    ))

  /** Set a workflow-level environment variable. */
  def env(key: String, value: String): CIWorkflow =
    copy(environment = environment.updated(key, value))

  /** Append a step; steps run serially in one VM, from the workspace root. */
  def step(name: String, command: String): CIWorkflow =
    copy(steps = steps :+ CIStep(name, command))

  /** Append a step with step-scoped environment variables. */
  def step(name: String, command: String, env: Map[String, String]): CIWorkflow =
    copy(steps = steps :+ CIStep(name, command, SortedMap.from(env)))

  /** Set the clone depth (default 1). */
  def withCloneDepth(depth: Int): CIWorkflow = copy(cloneDepth = Some(depth))

  /** Skip the checkout entirely. */
  def skipClone: CIWorkflow = copy(cloneSkip = true)

  /** The workflow file name [[save]] writes: `<name>.yml`. */
  def fileName: String =
    if name.endsWith(".yml") || name.endsWith(".yaml") then name
    else s"$name.yml"

  /** Render the workflow file.
    *
    * Scalars are emitted as JSON strings — valid YAML by construction — and
    * commands as literal blocks when safe, so the SDK needs no YAML
    * dependency.
    */
  def yaml: String =
    val sections = Vector.newBuilder[String]

    if when.nonEmpty then
      val lines = when.flatMap { case (events, branches) =>
        val head = s"  - event: [${events.map(q).mkString(", ")}]"
        branches.toList match
          case Nil        => Vector(head)
          case one :: Nil => Vector(head, s"    branch: ${q(one)}")
          case many => Vector(head, s"    branch: [${many.map(q).mkString(", ")}]")
      }
      sections += ("when:" +: lines).mkString("\n")

    sections += s"engine: $engine"

    if dependencies.nonEmpty then
      val lines = dependencies.toVector.flatMap { case (reg, pkgs) =>
        s"  ${q(reg)}:" +: pkgs.map(p => s"    - ${q(p)}")
      }
      sections += ("dependencies:" +: lines).mkString("\n")

    if environment.nonEmpty then
      val lines = environment.toVector.map { case (k, v) => s"  $k: ${q(v)}" }
      sections += ("environment:" +: lines).mkString("\n")

    if cloneSkip || cloneDepth.nonEmpty then
      val lines = Vector("clone:") ++
        (if cloneSkip then Vector("  skip: true") else Vector.empty) ++
        cloneDepth.map(d => s"  depth: $d").toVector
      sections += lines.mkString("\n")

    val stepLines = steps.flatMap { s =>
      Vector(s"  - name: ${q(s.name)}") ++ commandLines(s.command) ++
        (if s.env.nonEmpty then
           "    environment:" +: s.env.toVector.map { case (k, v) =>
             s"      $k: ${q(v)}"
           }
         else Vector.empty)
    }
    sections += ("steps:" +: stepLines).mkString("\n")

    sections.result().mkString("\n\n") + "\n"

  /** A literal block when it round-trips byte-for-byte; a JSON string when it
    * cannot (trailing spaces, carriage returns) — never a silent alteration.
    */
  private def commandLines(command: String): Vector[String] =
    val blockSafe = command.nonEmpty &&
      !command.contains('\r') &&
      command.split("\n", -1).forall(l => l == l.replaceAll(" +$", ""))
    if !blockSafe then Vector(s"    command: ${q(command)}")
    else
      "    command: |" +: command
        .replaceAll("\n+$", "")
        .split("\n", -1)
        .toVector
        .map(l => s"      $l")

  /** A JSON string literal, which is a valid YAML scalar by construction. */
  private def q(s: String): String =
    val sb = StringBuilder("\"")
    s.foreach {
      case '"'           => sb.append("\\\"")
      case '\\'          => sb.append("\\\\")
      case '\n'          => sb.append("\\n")
      case '\r'          => sb.append("\\r")
      case '\t'          => sb.append("\\t")
      case c if c < 0x20 => sb.append(f"\\u${c.toInt}%04x")
      case c             => sb.append(c)
    }
    sb.append('"').toString

  /** Write into `<repo>/.tangled/workflows/` and return the path. */
  def save(repo: Path): Path =
    val dir = repo.resolve(".tangled").resolve("workflows")
    Files.createDirectories(dir)
    val path = dir.resolve(fileName)
    Files.writeString(
      path,
      "# Generated by the bsdkrun SDK — edit the code that save()d it instead.\n" + yaml
    )
    path

  /** Execute the workflow in a microVM, streaming output.
    *
    * The YAML never touches the repository — it goes to a temp file and
    * `bsdkrun ci run -f`. Returns the failing step's error, or unit.
    */
  def run(dir: Option[Path] = None): Either[BsdkrunError, Unit] =
    val tmp = Files.createTempDirectory("bsdkrun-ci-")
    val file = tmp.resolve(fileName)
    Files.writeString(file, yaml)
    val args = Seq("ci", "run", "-f", file.toString) ++
      dir.toSeq.flatMap(d => Seq("-w", d.toString))
    val result = Proc.spawn(args)
    Files.deleteIfExists(file)
    Files.deleteIfExists(tmp)
    result.flatMap {
      case 0 => Right(())
      case code =>
        Left(
          BsdkrunError.CommandFailed(
            command = s"bsdkrun ci run ($name)",
            exitCode = code,
            stdout = "",
            stderr = s"workflow $name failed"
          )
        )
    }

final case class CIStep(
    name: String,
    command: String,
    env: SortedMap[String, String] = SortedMap.empty
)
