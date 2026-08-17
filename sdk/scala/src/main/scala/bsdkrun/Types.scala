package bsdkrun

import ujson.Value

/** Typed records mirroring `bsdkrun`'s `--json` output.
  *
  * Decoding is lenient and hand-written rather than derived: the same records
  * also back [[Client]], whose GraphQL responses are camelCase and send 64-bit
  * integers as strings (GraphQL has no `Int64`). Reading through `ujson.Value`
  * means a field the CLI adds later is ignored instead of failing the decode.
  */
object Types:

  /** One `ports` entry of a `ps --json` row. */
  final case class PortForward(bind: String, host: Int, guest: Int)

  /** A machine, as `bsdkrun ps --json` reports it. */
  final case class SandboxInfo(
      id: String,
      name: Option[String],
      image: String,
      kind: String,
      command: String,
      running: Boolean,
      exitCode: Option[Int],
      pid: Option[Int],
      detached: Boolean,
      cpus: Int,
      mem: Int,
      volume: Option[String],
      stateDir: String,
      network: Option[String],
      netIp: Option[String],
      ports: Seq[PortForward],
      createdAt: Option[Long],
      finishedAt: Option[Long],
      /** The snapshot this machine was branched from, if any. */
      origin: Option[String] = None
  ):
    /** `"running"` or `"exited"`, the way the CLI's table renders it. */
    def status: String = if running then "running" else "exited"

  /** A coding agent bsdkrun can sandbox.
    *
    * Each runs in a disposable microVM with a persistent login, a shared skills
    * store, and only the folder you choose to share.
    */
  final case class AiAgent(
      id: String,
      label: String,
      /** The catalog flavor that installs it. */
      flavor: String,
      description: String,
      /** Its flavor is provisioned, so a sandbox boots in a second. */
      installed: Boolean,
      running: Long
  )

  /** One agent sandbox. It is a machine, so `logs`/`stop` work on `id`. */
  final case class AiSession(
      id: String,
      name: String,
      agent: String,
      running: Boolean,
      /** The directory shared into it, on the engine's host. */
      workspace: Option[String],
      createdAt: Option[Long]
  )

  /** The Docker engine VM: whether it is up, and how to reach it.
    *
    * bsdkrun runs one `docker:dind` microVM and serves its API on a host unix
    * socket, so the host's own `docker` CLI drives the same engine.
    */
  final case class DockerStatus(
      running: Boolean,
      machineId: Option[String],
      machineRunning: Boolean,
      /** The unix socket the `docker` CLI talks to. */
      socket: String,
      socketReady: Boolean,
      apiPort: Option[Int],
      version: Option[String],
      containers: Option[Long],
      images: Option[Long],
      /** Host directories shared into the VM, each `HOST:GUEST`. */
      mounts: Seq[String],
      /** The dedicated image-store disk, when the VM has one. */
      disk: Option[String],
      /** Its size in bytes — sparse, so the cap rather than the usage. */
      diskSize: Option[Long]
  )

  /** A container in the Docker engine VM — a trimmed `docker ps` row. */
  final case class DockerContainer(
      id: String,
      name: String,
      image: String,
      command: String,
      /** `"running"`, `"exited"`, `"created"`, `"paused"`, ... */
      state: String,
      /** Docker's human status, e.g. `"Up 3 minutes"`. */
      status: String,
      /** Published forwards, each `HOST:GUEST/proto`. */
      ports: Seq[String],
      /** Unix epoch seconds. */
      created: Option[Long]
  ):
    /** Whether the container is up. */
    def isRunning: Boolean = state == "running"

  /** A machine snapshot: one machine's disk state, captured under a name.
    *
    * A copy-on-write clone rather than a memory image — the files the guest
    * wrote, not what it was executing. [[Client.branch]] boots a new machine
    * from one; [[Client.restoreMachine]] puts one back over the machine it
    * came from.
    */
  final case class SnapshotInfo(
      id: String,
      name: String,
      machineId: String,
      /** The machine's name when it was taken; empty if it had none. */
      machineName: String,
      /** `"linux"`, `"freebsd"`, `"netbsd"` or `"unikraft"`. */
      kind: String,
      image: String,
      path: String,
      /** The snapshot the source machine was itself branched from, if any. */
      parent: Option[String],
      description: String,
      cpus: Int,
      mem: Int,
      ports: Seq[PortForward],
      /** Human-readable, when measured — a CoW clone costs nothing to take. */
      size: Option[String],
      createdAt: Option[Long]
  )

  /** A downloaded image, as `bsdkrun images --json` reports it. */
  final case class ImageInfo(
      name: String,
      kind: String,
      size: String,
      path: String,
      digest: Option[String]
  )

  /** A persistent volume, as `bsdkrun volume ls --json` reports it. */
  final case class VolumeInfo(
      name: String,
      guest: Option[String],
      base: Option[String],
      path: String,
      size: String,
      createdAt: Option[Long],
      tracked: Boolean
  )

  /** A global network, as `bsdkrun network ls --json` reports it. */
  final case class NetworkInfo(
      name: String,
      subnet: Option[String],
      gateway: Option[String],
      members: Int,
      createdAt: Option[Long]
  )

  // -- lenient accessors ------------------------------------------------------
  //
  // Every field is read through these rather than `apply`, so a missing key is
  // a default instead of an exception, and a number sent as a JSON string —
  // which is how GraphQL has to send anything 64-bit — still parses.

  private[bsdkrun] def str(v: Value, key: String): String =
    optStr(v, key).getOrElse("")

  private[bsdkrun] def optStr(v: Value, key: String): Option[String] =
    v.objOpt.flatMap(_.get(key)).flatMap:
      case ujson.Str(s) => Some(s)
      case ujson.Null   => None
      case other        => Some(other.toString)

  private[bsdkrun] def bool(v: Value, key: String): Boolean =
    v.objOpt.flatMap(_.get(key)).flatMap(_.boolOpt).getOrElse(false)

  private[bsdkrun] def optLong(v: Value, key: String): Option[Long] =
    v.objOpt.flatMap(_.get(key)).flatMap:
      case ujson.Num(n) => Some(n.toLong)
      case ujson.Str(s) => s.trim.toLongOption
      case _            => None

  private[bsdkrun] def optInt(v: Value, key: String): Option[Int] =
    optLong(v, key).map(_.toInt)

  private[bsdkrun] def int(v: Value, key: String, default: Int = 0): Int =
    optInt(v, key).getOrElse(default)

  // -- decoders ---------------------------------------------------------------

  def portForward(v: Value): PortForward =
    PortForward(str(v, "bind"), int(v, "host"), int(v, "guest"))

  def sandboxInfo(v: Value): SandboxInfo =
    SandboxInfo(
      id = str(v, "id"),
      name = optStr(v, "name"),
      image = str(v, "image"),
      kind = str(v, "kind"),
      command = str(v, "command"),
      running = bool(v, "running"),
      exitCode = optInt(v, "exit_code").orElse(optInt(v, "exitCode")),
      pid = optInt(v, "pid"),
      detached = bool(v, "detached"),
      cpus = int(v, "cpus"),
      mem = int(v, "mem"),
      volume = optStr(v, "volume"),
      stateDir = optStr(v, "state_dir").orElse(optStr(v, "stateDir")).getOrElse(""),
      network = optStr(v, "network"),
      netIp = optStr(v, "net_ip").orElse(optStr(v, "netIp")),
      ports = v.objOpt
        .flatMap(_.get("ports"))
        .flatMap(_.arrOpt)
        .map(_.map(portForward).toSeq)
        .getOrElse(Seq.empty),
      createdAt = optLong(v, "created_at").orElse(optLong(v, "createdAt")),
      finishedAt = optLong(v, "finished_at").orElse(optLong(v, "finishedAt")),
      origin = optStr(v, "origin")
    )

  /** A GraphQL `AiAgent`, or an `ai agents --json` row. */
  def aiAgent(v: Value): AiAgent =
    AiAgent(
      id = str(v, "id"),
      label = str(v, "label"),
      flavor = str(v, "flavor"),
      description = str(v, "description"),
      installed = bool(v, "installed"),
      running = optLong(v, "running").getOrElse(0L)
    )

  /** A GraphQL `AiSession`, or an `ai ls --json` row. */
  def aiSession(v: Value): AiSession =
    AiSession(
      id = str(v, "id"),
      name = str(v, "name"),
      agent = str(v, "agent"),
      running = bool(v, "running"),
      workspace = optStr(v, "workspace"),
      createdAt = optLong(v, "created_at").orElse(optLong(v, "createdAt"))
    )

  /** A GraphQL `DockerStatus`, or a `docker status --json` row — both spellings
    * are accepted, as everywhere else here.
    */
  def dockerStatus(v: Value): DockerStatus =
    DockerStatus(
      running = bool(v, "running"),
      machineId = optStr(v, "machine_id").orElse(optStr(v, "machineId")),
      machineRunning = bool(v, "machine_running") || bool(v, "machineRunning"),
      socket = str(v, "socket"),
      socketReady = bool(v, "socket_ready") || bool(v, "socketReady"),
      apiPort = optInt(v, "api_port").orElse(optInt(v, "apiPort")),
      version = optStr(v, "version"),
      containers = optLong(v, "containers"),
      images = optLong(v, "images"),
      mounts = strings(v, "mounts"),
      disk = optStr(v, "disk"),
      diskSize = optLong(v, "disk_size").orElse(optLong(v, "diskSize"))
    )

  /** A GraphQL `DockerContainer`, or a `docker ps --json` row. */
  def dockerContainer(v: Value): DockerContainer =
    DockerContainer(
      id = str(v, "id"),
      name = str(v, "name"),
      image = str(v, "image"),
      command = str(v, "command"),
      state = str(v, "state"),
      status = str(v, "status"),
      ports = strings(v, "ports"),
      created = optLong(v, "created")
    )

  /** A string array field, empty when absent. */
  private def strings(v: Value, key: String): Seq[String] =
    v.objOpt
      .flatMap(_.get(key))
      .flatMap(_.arrOpt)
      .map(_.flatMap(_.strOpt).toSeq)
      .getOrElse(Seq.empty)

  /** A GraphQL `Snapshot` (camelCase) or a `snapshots --json` row (snake_case)
    * — both are accepted, as everywhere else here.
    */
  def snapshotInfo(v: Value): SnapshotInfo =
    SnapshotInfo(
      id = str(v, "id"),
      name = str(v, "name"),
      machineId = optStr(v, "machine_id").orElse(optStr(v, "machineId")).getOrElse(""),
      machineName = optStr(v, "machine_name").orElse(optStr(v, "machineName")).getOrElse(""),
      kind = str(v, "kind"),
      image = str(v, "image"),
      path = str(v, "path"),
      parent = optStr(v, "parent"),
      description = str(v, "description"),
      cpus = int(v, "cpus"),
      mem = int(v, "mem"),
      ports = v.objOpt
        .flatMap(_.get("ports"))
        .flatMap(_.arrOpt)
        .map(_.map(portForward).toSeq)
        .getOrElse(Seq.empty),
      size = optStr(v, "size"),
      createdAt = optLong(v, "created_at").orElse(optLong(v, "createdAt"))
    )

  def imageInfo(v: Value): ImageInfo =
    ImageInfo(
      name = str(v, "name"),
      kind = str(v, "kind"),
      size = str(v, "size"),
      path = str(v, "path"),
      digest = optStr(v, "digest")
    )

  def volumeInfo(v: Value): VolumeInfo =
    VolumeInfo(
      name = str(v, "name"),
      guest = optStr(v, "guest"),
      base = optStr(v, "base"),
      path = str(v, "path"),
      size = str(v, "size"),
      createdAt = optLong(v, "created_at").orElse(optLong(v, "createdAt")),
      tracked = bool(v, "tracked")
    )

  def networkInfo(v: Value): NetworkInfo =
    NetworkInfo(
      name = str(v, "name"),
      subnet = optStr(v, "subnet"),
      gateway = optStr(v, "gateway"),
      members = int(v, "members"),
      createdAt = optLong(v, "created_at").orElse(optLong(v, "createdAt"))
    )

  /** Parse a `--json` array into rows, or fail with the raw text. */
  def rows[A](raw: String, label: String, decode: Value => A): Either[BsdkrunError, Seq[A]] =
    val payload = if raw.trim.isEmpty then "[]" else raw.trim
    try
      ujson.read(payload).arrOpt match
        case Some(arr) => Right(arr.map(decode).toSeq)
        case None      => Left(BsdkrunError.DecodeFailed(label, raw))
    catch case _: Exception => Left(BsdkrunError.DecodeFailed(label, raw))

  /** Parse a `--json` object, or fail with the raw text. */
  def one(raw: String, label: String): Either[BsdkrunError, Value] =
    val payload = if raw.trim.isEmpty then "{}" else raw.trim
    try
      val parsed = ujson.read(payload)
      if parsed.objOpt.isDefined then Right(parsed)
      else Left(BsdkrunError.DecodeFailed(label, raw))
    catch case _: Exception => Left(BsdkrunError.DecodeFailed(label, raw))
