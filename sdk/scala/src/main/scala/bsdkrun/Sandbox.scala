package bsdkrun

import bsdkrun.Args.{CreateOptions, NetOptions, Os}
import bsdkrun.Types.SandboxInfo

/** A handle to a running (or stopped) bsdkrun microVM.
  *
  * Create one with the per-kind builders ([[Sandbox.linux]],
  * [[Sandbox.freebsd]], ...), reconnect with [[Sandbox.get]], or enumerate with
  * [[Sandbox.list]].
  *
  * {{{
  * for
  *   sbx <- Sandbox.linux("alpine").cpus(2).command("sleep", "300").create()
  *   out <- sbx.exec("uname", "-a")
  *   _   <- sbx.stop()
  * yield out.text
  * }}}
  */
final class Sandbox private[bsdkrun] (val id: String, val sshPort: Option[Int]):

  /** Read and write files in the guest. */
  lazy val fs: FileSystem = new FileSystem(id)

  /** Save and restore guest directories under a key. */
  lazy val cache: Cache = new Cache(id)

  override def toString: String = s"Sandbox($id)"

  // -- commands ---------------------------------------------------------------

  /** Run a command in the guest through its exec agent.
    *
    * The primary programmatic entrypoint: argv goes straight through, with no
    * shell parsing. Use [[sh]] when a shell is what you want.
    */
  def exec(command: Seq[String], opts: ExecOptions): Either[BsdkrunError, CommandResult] =
    val argv =
      if opts.cwd.isEmpty then command
      else
        // Emulate a working directory: cd, drop it, then exec the real argv.
        Seq("/bin/sh", "-c", "cd \"$1\" && shift && exec \"$@\"", "sh", opts.cwd.get) ++ command

    val args = Seq.newBuilder[String]
    args += "exec"
    if opts.tty then args += "-t"
    opts.env.toSeq.sortBy(_._1).foreach((k, v) => args ++= Seq("-e", s"$k=$v"))
    args += id
    args ++= argv

    Proc
      .run(
        args.result(),
        Proc.Options(
          stdin = opts.stdin,
          logLevel = opts.logLevel,
          onStdout = opts.onStdout,
          onStderr = opts.onStderr
        )
      )
      .map(r => CommandResult(r.stdout, r.stderr, r.exitCode, s"exec ${argv.mkString(" ")}"))

  /** Run a command with no extra options. */
  def exec(command: String, rest: String*): Either[BsdkrunError, CommandResult] =
    exec(command +: rest, ExecOptions())

  /** Run a shell script in the guest. Pair with the `sh` interpolator for
    * quoting.
    */
  def sh(script: String, opts: ExecOptions = ExecOptions()): Either[BsdkrunError, CommandResult] =
    exec(Seq("/bin/sh", "-c", script), opts)

  /** Read the machine's console log, or bsdkrun's own boot log. */
  def logs(boot: Boolean = false): Either[BsdkrunError, String] =
    val args = Seq("logs") ++ (if boot then Seq("--boot") else Nil) ++ Seq(id)
    Proc.run(args).map(_.stdout)

  /** Attach an interactive shell, inheriting this process's terminal. Blocks
    * until the shell exits and returns its exit code.
    */
  def shell(): Either[BsdkrunError, Int] = Proc.spawn(Seq("shell", id))

  // -- inspection -------------------------------------------------------------

  /** This machine's current row, or `None` if it is gone. */
  def status(): Either[BsdkrunError, Option[SandboxInfo]] =
    Sandbox.list(all = true).map(_.find(_.id == id))

  def isRunning(): Either[BsdkrunError, Boolean] =
    status().map(_.exists(_.running))

  // -- lifecycle --------------------------------------------------------------

  /** Stop the machine. BSD guests are cleanly powered off; Linux is SIGTERM'd. */
  def stop(): Either[BsdkrunError, Sandbox] = lifecycle(Seq("stop", id), "bsdkrun stop")

  /** Restart a stopped machine in place — same id, disk/rootfs and network. */
  def start(): Either[BsdkrunError, Sandbox] = lifecycle(Seq("start", id), "bsdkrun start")

  /** Remove the machine and its state. `force` stops it first if running. */
  def remove(force: Boolean = false): Either[BsdkrunError, Unit] =
    val args = Seq("rm") ++ (if force then Seq("--force") else Nil) ++ Seq(id)
    Proc.runChecked(args, "bsdkrun rm").map(_ => ())

  /** Change the recorded vCPU / RAM. libkrun fixes resources at boot, so this
    * applies on the next [[start]].
    */
  def update(cpus: Option[Int] = None, mem: Option[Int] = None): Either[BsdkrunError, Sandbox] =
    val args = Seq("update") ++
      cpus.toSeq.flatMap(c => Seq("--cpus", c.toString)) ++
      mem.toSeq.flatMap(m => Seq("--mem", m.toString)) ++ Seq(id)
    lifecycle(args, "bsdkrun update")

  /** Join a global network. Takes effect on the next [[start]]. */
  def connectNetwork(network: String): Either[BsdkrunError, Sandbox] =
    lifecycle(Seq("network", "connect", network, id), "bsdkrun network connect")

  /** Leave the machine's global network. */
  def disconnectNetwork(): Either[BsdkrunError, Sandbox] =
    lifecycle(Seq("network", "disconnect", id), "bsdkrun network disconnect")

  /** Snapshot the machine's current state into a named flavor. */
  def commit(name: String, description: Option[String] = None): Either[BsdkrunError, Unit] =
    val args = Seq("commit") ++
      description.toSeq.flatMap(d => Seq("-d", d)) ++ Seq(id, name)
    Proc.runChecked(args, "bsdkrun commit").map(_ => ())

  /** Lifecycle calls return the sandbox itself, so they chain. */
  private def lifecycle(args: Seq[String], label: String): Either[BsdkrunError, Sandbox] =
    Proc.runChecked(args, label).map(_ => this)

/** Extra options for [[Sandbox.exec]]. */
final case class ExecOptions(
    /** Environment variables for this command only (`-e K=V`). */
    env: Map[String, String] = Map.empty,
    /** Allocate a pseudo-TTY in the guest (`-t`). */
    tty: Boolean = false,
    /** Data piped to the command's stdin. */
    stdin: Option[Array[Byte]] = None,
    /** Working directory inside the guest (emulated via `sh -c 'cd …'`). */
    cwd: Option[String] = None,
    /** Per-command bsdkrun log level (default 0 — quiet). */
    logLevel: Int = 0,
    /** Receive stdout as it arrives; it is still captured in the result. */
    onStdout: Option[Array[Byte] => Unit] = None,
    onStderr: Option[Array[Byte] => Unit] = None
)

object Sandbox:

  /** A machine id as the CLI prints it: lowercase hex on a line of its own. */
  private val IdPattern = "^[0-9a-f]{6,}$".r
  private val SshPortPattern = raw"ssh -p (\d+)".r

  /** Boot an OCI image as a Linux microVM. */
  def linux(image: String): Builder = Builder(CreateOptions(Os.Linux, image = Some(image)))

  /** Boot FreeBSD (EFI on macOS, PVH on Linux/amd64). */
  def freebsd(): Builder = Builder(CreateOptions(Os.Freebsd))

  /** Boot NetBSD (direct-kernel boot everywhere). */
  def netbsd(): Builder = Builder(CreateOptions(Os.Netbsd))

  /** Boot a raw disk through a UEFI firmware image. */
  def firmware(firmware: String, disk: String): Builder =
    Builder(CreateOptions(Os.Firmware, firmware = Some(firmware), disk = Some(disk)))

  /** Boot a kernel directly, with no bootloader. */
  def kernel(kernel: String): Builder = Builder(CreateOptions(Os.Kernel, kernel = Some(kernel)))

  /** Boot a Unikraft unikernel. `path` is a kraft project dir or an image. */
  def unikraft(path: String = "."): Builder = Builder(CreateOptions(Os.Unikraft, path = Some(path)))

  /** Boot a Solo5 (MirageOS) unikernel under the `solo5-hvt` tender. */
  def solo5(path: String = "."): Builder = Builder(CreateOptions(Os.Solo5, path = Some(path)))

  /** Boot a Nanos unikernel image. */
  def nanos(image: String): Builder = Builder(CreateOptions(Os.Nanos, image = Some(image)))

  /** Boot an OSv unikernel image. */
  def osv(image: String): Builder = Builder(CreateOptions(Os.Osv, image = Some(image)))

  /** Start from raw options, for a caller building them programmatically. */
  def builder(opts: CreateOptions): Builder = Builder(opts)

  /** Reconnect to an existing machine by id, name, or a unique id prefix. */
  def get(id: String): Either[BsdkrunError, Sandbox] =
    list(all = true).flatMap: rows =>
      rows
        .find(m => m.id == id || m.id.startsWith(id) || m.name.contains(id))
        .map(m => new Sandbox(m.id, None))
        .toRight(BsdkrunError.SandboxNotFound(id))

  /** List machines. `all` includes exited ones (default: running only). */
  def list(all: Boolean = false): Either[BsdkrunError, Seq[SandboxInfo]] =
    val args = Seq("ps", "--json") ++ (if all then Seq("--all") else Nil)
    Proc
      .runChecked(args, "bsdkrun ps")
      .flatMap(res => Types.rows(res.stdout, "bsdkrun ps", Types.sandboxInfo))

  /** Boot a machine from options and return a handle to it. */
  private[bsdkrun] def create(opts: CreateOptions): Either[BsdkrunError, Sandbox] =
    for
      argv <- Args.build(opts)
      res <- Proc.run(argv, Proc.Options(logLevel = opts.logLevel.getOrElse(1)))
      sbx <- fromCreateOutput(res)
    yield sbx

  private def fromCreateOutput(res: Proc.RawResult): Either[BsdkrunError, Sandbox] =
    if res.exitCode != 0 then
      Left(BsdkrunError.CommandFailed(res.exitCode, res.stdout, res.stderr, "bsdkrun create"))
    else
      // A detached run prints just the machine id on stdout; take the last line
      // that looks like one, since a boot can log above it.
      res.stdout.linesIterator
        .map(_.trim)
        .filter(IdPattern.matches)
        .toSeq
        .lastOption
        .map: id =>
          val port = SshPortPattern.findFirstMatchIn(res.stderr).flatMap(_.group(1).toIntOption)
          new Sandbox(id, port)
        .toRight(
          BsdkrunError.CommandFailed(
            res.exitCode,
            res.stdout,
            res.stderr,
            "bsdkrun create (no machine id in output)"
          )
        )

  /** Accumulates [[CreateOptions]] by pipe, then boots.
    *
    * Nothing is sent to `bsdkrun` until [[create]] runs, so a builder is worth
    * inspecting with [[toArgs]] in a test.
    */
  final case class Builder(opts: CreateOptions):

    // -- shared ---------------------------------------------------------------
    def name(name: String): Builder = copy(opts.copy(name = Some(name)))
    def cpus(n: Int): Builder = copy(opts.copy(cpus = Some(n)))
    def mem(mib: Int): Builder = copy(opts.copy(mem = Some(mib)))
    def logLevel(level: Int): Builder = copy(opts.copy(logLevel = Some(level)))

    /** Add a host->guest TCP forward, `"HOST:GUEST"`. */
    def port(forward: String): Builder =
      copy(opts.copy(net = opts.net.copy(ports = opts.net.ports :+ forward)))

    def port(host: Int, guest: Int): Builder = port(s"$host:$guest")
    def ports(forwards: String*): Builder = forwards.foldLeft(this)(_.port(_))
    def mac(mac: String): Builder = copy(opts.copy(net = opts.net.copy(mac = Some(mac))))
    def network(name: String): Builder = copy(opts.copy(net = opts.net.copy(network = Some(name))))
    def noNet: Builder = copy(opts.copy(net = opts.net.copy(disabled = true)))

    // -- linux ----------------------------------------------------------------
    def kernel(path: String): Builder = copy(opts.copy(kernel = Some(path)))
    def kernelVersion(v: String): Builder = copy(opts.copy(kernelVersion = Some(v)))
    def initramfs: Builder = copy(opts.copy(initramfs = true))
    def initramfsPath(path: String): Builder = copy(opts.copy(initramfsPath = Some(path)))
    def entrypoint(ep: String): Builder = copy(opts.copy(entrypoint = Some(ep)))
    def console(dev: String): Builder = copy(opts.copy(console = Some(dev)))
    def mount(spec: String): Builder = copy(opts.copy(mounts = opts.mounts :+ spec))
    def mounts(specs: String*): Builder = specs.foldLeft(this)(_.mount(_))
    def command(argv: String*): Builder = copy(opts.copy(command = argv))

    /** Set an environment variable for the guest's entrypoint (`-e K=V`).
      *
      * Merges rather than replaces, so it composes down a chain instead of the
      * last call winning. The variables are merged over the image's own config,
      * so a key the image already defines is replaced rather than duplicated.
      */
    def env(key: String, value: String): Builder =
      copy(opts.copy(env = opts.env.filterNot(_._1 == key) :+ (key -> value)))

    /** Set several environment variables at once. */
    def envs(vars: Map[String, String]): Builder =
      vars.foldLeft(this)((b, kv) => b.env(kv._1, kv._2))

    // -- disks ----------------------------------------------------------------
    def persist: Builder = copy(opts.copy(persist = true))
    def volume(name: String): Builder = copy(opts.copy(volume = Some(name)))
    def attachDisk(spec: String): Builder = copy(opts.copy(attachDisk = opts.attachDisk :+ spec))

    // -- bsd / firmware / kernel / unikernels ---------------------------------
    def version(v: String): Builder = copy(opts.copy(version = Some(v)))
    def firmware(path: String): Builder = copy(opts.copy(firmware = Some(path)))
    def force: Builder = copy(opts.copy(force = true))
    def disk(path: String): Builder = copy(opts.copy(disk = Some(path)))
    def format(fmt: String): Builder = copy(opts.copy(format = Some(fmt)))
    def cmdline(line: String): Builder = copy(opts.copy(cmdline = Some(line)))
    def gic(version: String): Builder = copy(opts.copy(gic = Some(version)))
    def block(spec: String): Builder = copy(opts.copy(block = opts.block :+ spec))
    def guestArgs(args: String*): Builder = copy(opts.copy(guestArgs = args))

    /** Any option not wrapped above — the escape hatch. */
    def withOptions(f: CreateOptions => CreateOptions): Builder = copy(f(opts))

    /** The exact argv [[create]] will run, for inspection and tests. */
    def toArgs: Either[BsdkrunError, Seq[String]] = Args.build(opts)

    /** Boot the machine (detached) and return a handle to it. */
    def create(): Either[BsdkrunError, Sandbox] = Sandbox.create(opts)

    /** Boot the machine, throwing [[BsdkrunException]] on failure. */
    def createOrThrow(): Sandbox = create().fold(e => throw BsdkrunException(e), identity)
