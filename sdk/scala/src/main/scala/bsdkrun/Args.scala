package bsdkrun

/** Builds the `bsdkrun` argv for a detached `create`, one shape per guest kind.
  *
  * Nothing here runs anything — [[CreateOptions]] is data, and [[Args.build]]
  * is a pure function of it. That is what lets the tests assert on a command
  * line without a machine anywhere near them.
  */
object Args:

  /** The guest kinds `bsdkrun` can boot. */
  enum Os:
    case Linux, Freebsd, Netbsd, Firmware, Kernel, Unikraft, Solo5, Nanos, Osv

  object Os:
    /** Parse the CLI's spelling, for callers holding a string. */
    def fromString(s: String): Either[BsdkrunError, Os] =
      Os.values
        .find(_.toString.equalsIgnoreCase(s))
        .toRight(
          BsdkrunError.InvalidOptions(
            s"unknown os \"$s\" — expected one of: ${Os.values.map(_.toString.toLowerCase).mkString(", ")}"
          )
        )

  /** Networking flags, shared by every guest kind. */
  final case class NetOptions(
      disabled: Boolean = false,
      /** Host->guest TCP forwards, each `"HOST:GUEST"` or `"BIND:HOST:GUEST"`. */
      ports: Seq[String] = Seq.empty,
      mac: Option[String] = None,
      network: Option[String] = None
  )

  /** Everything `create` can be told, across every guest kind.
    *
    * One record rather than nine: the flags genuinely overlap, and the CLI is
    * the authority on which combination is valid. [[Args.build]] checks only
    * what a kind cannot do without — an image, a kernel, a firmware+disk pair —
    * so an unsupported flag is reported by `bsdkrun` itself rather than
    * silently dropped here.
    */
  final case class CreateOptions(
      os: Os,
      // linux / nanos / osv
      image: Option[String] = None,
      // linux / kernel / nanos
      kernel: Option[String] = None,
      kernelVersion: Option[String] = None,
      /** Linux: the bare `--initramfs` flag. kernel/unikraft: a path. */
      initramfs: Boolean = false,
      initramfsPath: Option[String] = None,
      entrypoint: Option[String] = None,
      /** Guest environment for the entrypoint (`-e K=V`, Linux). */
      env: Seq[(String, String)] = Seq.empty,
      console: Option[String] = None,
      mounts: Seq[String] = Seq.empty,
      command: Seq[String] = Seq.empty,
      // freebsd / netbsd
      version: Option[String] = None,
      firmware: Option[String] = None,
      force: Boolean = false,
      // kernel / firmware / osv
      disk: Option[String] = None,
      format: Option[String] = None,
      cmdline: Option[String] = None,
      gic: Option[String] = None,
      // unikraft / solo5
      path: Option[String] = None,
      block: Seq[String] = Seq.empty,
      guestArgs: Seq[String] = Seq.empty,
      // disk persistence (BSD / firmware / kernel / nanos / osv)
      persist: Boolean = false,
      volume: Option[String] = None,
      attachDisk: Seq[String] = Seq.empty,
      // shared
      net: NetOptions = NetOptions(),
      name: Option[String] = None,
      cpus: Option[Int] = None,
      mem: Option[Int] = None,
      /** bsdkrun's global `--log-level` for the create call. Defaults to 1, so
        * boot diagnostics land in the error when a boot fails.
        */
      logLevel: Option[Int] = None
  )

  // -- shared flag groups -----------------------------------------------------

  private def netArgs(net: NetOptions): Seq[String] =
    val b = Seq.newBuilder[String]
    if net.disabled then b += "--no-net"
    net.ports.foreach(p => b ++= Seq("--port", p))
    net.mac.foreach(m => b ++= Seq("--mac", m))
    net.network.foreach(n => b ++= Seq("--network", n))
    b.result()

  private def nameArgs(o: CreateOptions): Seq[String] =
    o.name.toSeq.flatMap(n => Seq("--name", n))

  private def vmArgs(o: CreateOptions): Seq[String] =
    o.cpus.toSeq.flatMap(c => Seq("--cpus", c.toString)) ++
      o.mem.toSeq.flatMap(m => Seq("--mem", m.toString))

  private def diskArgs(o: CreateOptions): Seq[String] =
    val b = Seq.newBuilder[String]
    if o.persist then b += "--persist"
    o.volume.foreach(v => b ++= Seq("-v", v))
    o.attachDisk.foreach(d => b ++= Seq("--attach-disk", d))
    b.result()

  /** `-e K=V` per entry, **sorted by key**.
    *
    * Callers add variables in whatever order suits them, so sorting is what
    * makes the argv — and the tests that assert on it — deterministic. The
    * guest sees the same environment either way.
    */
  private def envArgs(env: Seq[(String, String)]): Seq[String] =
    env.sortBy(_._1).flatMap((k, v) => Seq("-e", s"$k=$v"))

  private def tail(o: CreateOptions): Seq[String] =
    netArgs(o.net) ++ nameArgs(o) ++ vmArgs(o)

  private def require(value: Option[String], field: String, os: Os): Either[BsdkrunError, String] =
    value
      .filter(_.nonEmpty)
      .toRight(BsdkrunError.InvalidOptions(s"${os.toString.toLowerCase} guests need a non-empty $field"))

  // -- per-kind builders ------------------------------------------------------

  /** Build the full detached `create` argv (minus the binary and global flags). */
  def build(o: CreateOptions): Either[BsdkrunError, Seq[String]] =
    o.os match
      case Os.Linux =>
        require(o.image, "image", o.os).map: image =>
          val b = Seq.newBuilder[String]
          b ++= Seq("linux", image, "-d")
          o.kernel.foreach(k => b ++= Seq("--kernel", k))
          o.kernelVersion.foreach(v => b ++= Seq("--kernel-version", v))
          if o.initramfs then b += "--initramfs"
          o.volume.foreach(v => b ++= Seq("-v", v))
          o.mounts.foreach(m => b ++= Seq("--mount", m))
          o.attachDisk.foreach(d => b ++= Seq("--attach-disk", d))
          o.entrypoint.foreach(e => b ++= Seq("--entrypoint", e))
          b ++= envArgs(o.env)
          o.console.foreach(c => b ++= Seq("--console", c))
          b ++= tail(o)
          if o.command.nonEmpty then b ++= "--" +: o.command
          b.result()

      case Os.Freebsd =>
        val b = Seq.newBuilder[String]
        b ++= Seq("freebsd", "-d")
        o.version.foreach(v => b ++= Seq("--version", v))
        o.firmware.foreach(f => b ++= Seq("--firmware", f))
        if o.force then b += "--force"
        b ++= diskArgs(o) ++ tail(o)
        Right(b.result())

      case Os.Netbsd =>
        val b = Seq.newBuilder[String]
        b ++= Seq("netbsd", "-d")
        o.version.foreach(v => b ++= Seq("--version", v))
        if o.force then b += "--force"
        b ++= diskArgs(o) ++ tail(o)
        Right(b.result())

      case Os.Firmware =>
        for
          firmware <- require(o.firmware, "firmware", o.os)
          disk <- require(o.disk, "disk", o.os)
        yield Seq("firmware", "--firmware", firmware, "--disk", disk, "-d") ++
          diskArgs(o) ++ tail(o)

      case Os.Kernel =>
        require(o.kernel, "kernel", o.os).map: kernel =>
          val b = Seq.newBuilder[String]
          b ++= Seq("kernel", "--kernel", kernel, "-d")
          o.format.foreach(f => b ++= Seq("--format", f))
          o.initramfsPath.foreach(p => b ++= Seq("--initramfs", p))
          o.cmdline.foreach(c => b ++= Seq("--cmdline", c))
          o.disk.foreach(d => b ++= Seq("--disk", d))
          b ++= diskArgs(o) ++ tail(o)
          b.result()

      case Os.Unikraft =>
        // No disk flags: a unikernel has no disk to persist, attach or clone.
        // Volumes are the exception — virtio-fs shares need neither.
        val b = Seq.newBuilder[String]
        b ++= Seq("unikraft", "-d")
        o.cmdline.foreach(c => b ++= Seq("--cmdline", c))
        o.initramfsPath.foreach(p => b ++= Seq("--initramfs", p))
        o.mounts.foreach(m => b ++= Seq("--mount", m))
        b ++= tail(o)
        b += o.path.getOrElse(".")
        Right(b.result())

      case Os.Solo5 =>
        // The unikernel declares its own devices in an MFT1 note, so only the
        // block backing files are passed. Guest args go last behind a literal
        // `--`: MirageOS options look like bsdkrun's own (`--ipv4=...`).
        val b = Seq.newBuilder[String]
        b ++= Seq("solo5", "-d")
        o.block.foreach(bl => b ++= Seq("--block", bl))
        b ++= tail(o)
        b += o.path.getOrElse(".")
        if o.guestArgs.nonEmpty then b ++= "--" +: o.guestArgs
        Right(b.result())

      case Os.Nanos =>
        require(o.image, "image", o.os).map: image =>
          val b = Seq.newBuilder[String]
          b ++= Seq("nanos", "-d")
          o.kernel.foreach(k => b ++= Seq("--kernel", k))
          o.cmdline.foreach(c => b ++= Seq("--cmdline", c))
          if o.persist then b += "--persist"
          b ++= tail(o)
          b += image
          b.result()

      case Os.Osv =>
        require(o.image, "image", o.os).map: image =>
          val b = Seq.newBuilder[String]
          b ++= Seq("osv", "-d")
          o.cmdline.foreach(c => b ++= Seq("--cmdline", c))
          o.disk.foreach(d => b ++= Seq("--disk", d))
          o.gic.foreach(g => b ++= Seq("--gic", g))
          if o.persist then b += "--persist"
          b ++= tail(o)
          b += image
          b.result()
