package bsdkrun

import bsdkrun.Types.{ImageInfo, NetworkInfo, VolumeInfo}

/** Downloaded images — OCI images pulled and BSD images fetched. */
object Images:

  /** List downloaded images. */
  def list(): Either[BsdkrunError, Seq[ImageInfo]] =
    Proc
      .runChecked(Seq("images", "--json"), "bsdkrun images")
      .flatMap(res => Types.rows(res.stdout, "bsdkrun images", Types.imageInfo))

/** Persistent volumes. */
object Volumes:

  def list(): Either[BsdkrunError, Seq[VolumeInfo]] =
    Proc
      .runChecked(Seq("volume", "ls", "--json"), "bsdkrun volume ls")
      .flatMap(res => Types.rows(res.stdout, "bsdkrun volume ls", Types.volumeInfo))

  /** Remove volumes and their data. */
  def remove(names: Seq[String], force: Boolean = false): Either[BsdkrunError, Unit] =
    val args = Seq("volume", "rm") ++ (if force then Seq("--force") else Nil) ++ names
    Proc.runChecked(args, "bsdkrun volume rm").map(_ => ())

/** Global networks — a shared subnet plus internal DNS, so machines reach each
  * other by name.
  */
object Networks:

  def list(): Either[BsdkrunError, Seq[NetworkInfo]] =
    Proc
      .runChecked(Seq("network", "ls", "--json"), "bsdkrun network ls")
      .flatMap(res => Types.rows(res.stdout, "bsdkrun network ls", Types.networkInfo))

  def create(name: String): Either[BsdkrunError, Unit] =
    Proc.runChecked(Seq("network", "create", name), "bsdkrun network create").map(_ => ())

  def remove(names: Seq[String], force: Boolean = false): Either[BsdkrunError, Unit] =
    val args = Seq("network", "rm") ++ (if force then Seq("--force") else Nil) ++ names
    Proc.runChecked(args, "bsdkrun network rm").map(_ => ())

  /** Attach a machine to a network. Takes effect on its next start. */
  def connect(network: String, id: String): Either[BsdkrunError, Unit] =
    Proc.runChecked(Seq("network", "connect", network, id), "bsdkrun network connect").map(_ => ())

  def disconnect(id: String): Either[BsdkrunError, Unit] =
    Proc.runChecked(Seq("network", "disconnect", id), "bsdkrun network disconnect").map(_ => ())

  /** Re-apply a network's DNS and addressing to its running members. */
  def sync(name: String): Either[BsdkrunError, Unit] =
    Proc.runChecked(Seq("network", "sync", name), "bsdkrun network sync").map(_ => ())

/** Host-level toolchain and image operations.
  *
  * Named `Host` rather than `System` — which is what the other SDKs call it —
  * because `java.lang.System` is auto-imported into every Scala file. An
  * `object System` here would shadow it for anyone doing `import bsdkrun.*`,
  * so `System.currentTimeMillis()` would stop compiling in their code.
  */
object Host:

  /** Sanity-check the toolchain: verify libkrun links and a VM context can be
    * created and configured. Does not boot anything.
    */
  def probe(): Boolean = Proc.run(Seq("probe")).exists(_.exitCode == 0)

  /** Check that this host can run machines, and report what to fix if not.
    *
    * Returns the report as JSON — `ok` says whether anything failed, and
    * `checks` lists each one with a `fix` where there is something to do.
    */
  def doctor(): Either[BsdkrunError, ujson.Value] =
    Proc.run(Seq("doctor", "--json")).flatMap { res =>
      // `doctor` exits 1 when a check fails, which is the answer rather than a
      // failure to get one — so the report is parsed either way.
      Types.one(res.stdout, "bsdkrun doctor")
    }

  /** Download and prepare a BSD image ahead of time. */
  def fetchImage(
      os: String,
      version: Option[String] = None,
      force: Boolean = false
  ): Either[BsdkrunError, String] =
    val args = Seq("fetch", "--os", os) ++
      version.toSeq.flatMap(v => Seq("--version", v)) ++
      (if force then Seq("--force") else Nil)
    Proc.runChecked(args, "bsdkrun fetch").map(_.stdout)

  /** The builds available to fetch for a BSD, one per line. */
  def versions(os: String): Either[BsdkrunError, Seq[String]] =
    Proc
      .runChecked(Seq("versions", "--os", os), "bsdkrun versions")
      .map(_.stdout.linesIterator.map(_.trim).filter(_.nonEmpty).toSeq)

  /** Grow a raw disk image. Only ever enlarges the file. */
  def growDisk(disk: String, size: String): Either[BsdkrunError, Unit] =
    Proc.runChecked(Seq("grow", "--disk", disk, "--size", size), "bsdkrun grow").map(_ => ())
