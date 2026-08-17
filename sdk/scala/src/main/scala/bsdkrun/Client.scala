package bsdkrun

import java.io.ByteArrayOutputStream
import java.net.URI
import java.net.http.{HttpClient, HttpRequest, HttpResponse, WebSocket}
import java.nio.charset.StandardCharsets.UTF_8
import java.util.Base64
import java.util.concurrent.CompletionStage
import scala.collection.mutable
import scala.concurrent.duration.Duration
import ujson.Value

import bsdkrun.Types.{DockerContainer, DockerStatus, SandboxInfo, SnapshotInfo}

/** A client for a remote `bsdkrund` over its GraphQL API.
  *
  * Queries and mutations go over HTTP; subscriptions (log streaming, shell
  * sessions) over a single shared WebSocket speaking `graphql-transport-ws`.
  * Both come from `java.net.http`, so the SDK still has no transport
  * dependency.
  *
  * {{{
  * for
  *   client <- Client.fromEnv()
  *   rows   <- client.listMachines(all = true)
  * yield rows
  * }}}
  */
final class Client private (val url: String, token: String):

  private val http = HttpClient.newHttpClient().nn

  // WebSocket state. One socket is shared by every subscription and opened
  // lazily on the first one; `acked` gates sends until `connection_ack`.
  private object ws:
    var socket: Option[WebSocket] = None
    var acked = false
    var pending: List[String] = Nil
    val subs: mutable.Map[String, Subscriber] = mutable.Map.empty
    var nextId = 0

  private final case class Subscriber(
      onNext: Value => Unit,
      onError: BsdkrunError => Unit,
      onComplete: () => Unit
  )

  /** The websocket endpoint derived from the HTTP one. */
  def wsUrl: String = Client.wsUrl(url)

  // -- HTTP transport ---------------------------------------------------------

  /** Run an arbitrary query or mutation, returning `data`.
    *
    * Every typed method below is built on this; it is public as the escape
    * hatch for documents the SDK has no wrapper for yet.
    */
  def request(query: String, variables: Value = ujson.Obj()): Either[BsdkrunError, Value] =
    val body = ujson.write(ujson.Obj("query" -> query, "variables" -> variables))
    val req = HttpRequest
      .newBuilder(URI.create(url).nn)
      .nn
      .header("content-type", "application/json")
      .nn
      .header("authorization", s"Bearer $token")
      .nn
      .POST(HttpRequest.BodyPublishers.ofString(body).nn)
      .nn
      .build()
      .nn

    val response =
      try Right(http.send(req, HttpResponse.BodyHandlers.ofString().nn).nn)
      catch
        case e: Exception =>
          Left(BsdkrunError.GraphQL(s"cannot reach the bsdkrun daemon at $url — ${e.getMessage}"))

    response.flatMap: res =>
      if res.statusCode() == 401 then Left(BsdkrunError.Auth())
      else
        val parsed =
          try Some(ujson.read(res.body().nn))
          catch case _: Exception => None
        parsed match
          case None =>
            Left(BsdkrunError.GraphQL(s"the daemon returned a non-JSON response (${res.statusCode()})"))
          case Some(json) => Client.dataOrError(json)

  // -- WebSocket transport ----------------------------------------------------

  private def send(socket: WebSocket, text: String): Unit =
    val _ = socket.sendText(text, true)

  /** Subscribe to a GraphQL subscription. Returns a function that unsubscribes. */
  def subscribe(
      query: String,
      variables: Value,
      onNext: Value => Unit,
      onError: BsdkrunError => Unit = _ => (),
      onComplete: () => Unit = () => ()
  ): Either[BsdkrunError, () => Unit] =
    ensureSocket().map: socket =>
      val id = ws.synchronized:
        ws.nextId += 1
        val id = ws.nextId.toString
        ws.subs(id) = Subscriber(onNext, onError, onComplete)
        id

      val message = ujson.write(
        ujson.Obj(
          "id" -> id,
          "type" -> "subscribe",
          "payload" -> ujson.Obj("query" -> query, "variables" -> variables)
        )
      )
      // Queue until `connection_ack`: a `subscribe` sent before the ack is
      // discarded by the daemon, which reads as a subscription that silently
      // never delivers.
      ws.synchronized:
        if ws.acked then send(socket, message) else ws.pending = ws.pending :+ message

      () => unsubscribe(id)

  private def unsubscribe(id: String): Unit =
    val (had, socket, remaining) = ws.synchronized:
      val had = ws.subs.remove(id).isDefined
      (had, ws.socket, ws.subs.size)
    if had then
      socket.foreach(s => send(s, ujson.write(ujson.Obj("id" -> id, "type" -> "complete"))))
      if remaining == 0 then closeSocket()

  private def closeSocket(): Unit =
    val socket = ws.synchronized:
      val s = ws.socket
      ws.socket = None
      ws.acked = false
      ws.pending = Nil
      ws.subs.clear()
      s
    socket.foreach: s =>
      try { val _ = s.sendClose(WebSocket.NORMAL_CLOSURE, "") }
      catch case _: Exception => ()

  /** The socket closed. Whether `connection_ack` ever arrived decides the
    * reason: an unacked close means the daemon rejected the token; an acked
    * one is just a dropped connection.
    */
  private def handleDisconnect(): Unit =
    val (subs, wasAcked) = ws.synchronized:
      val snapshot = ws.subs.values.toList
      val acked = ws.acked
      ws.socket = None
      ws.acked = false
      ws.pending = Nil
      ws.subs.clear()
      (snapshot, acked)
    if subs.nonEmpty then
      val err =
        if wasAcked then BsdkrunError.GraphQL("the connection to the daemon was closed")
        else BsdkrunError.Auth()
      subs.foreach(s => try s.onError(err) catch case _: Exception => ())

  private def handleMessage(socket: WebSocket, raw: String): Unit =
    val msg =
      try ujson.read(raw)
      catch case _: Exception => ujson.Null
    Types.optStr(msg, "type").foreach:
      case "connection_ack" =>
        val queued = ws.synchronized:
          ws.acked = true
          val q = ws.pending
          ws.pending = Nil
          q
        queued.foreach(send(socket, _))

      case "next" =>
        for
          id <- Types.optStr(msg, "id")
          sub <- ws.synchronized(ws.subs.get(id))
          payload <- msg.objOpt.flatMap(_.get("payload"))
          data <- payload.objOpt.flatMap(_.get("data"))
        do try sub.onNext(data) catch case _: Exception => ()

      case "error" =>
        for
          id <- Types.optStr(msg, "id")
          sub <- ws.synchronized(ws.subs.remove(id))
        do
          val detail = msg.objOpt
            .flatMap(_.get("payload"))
            .flatMap(_.arrOpt)
            .flatMap(_.headOption)
            .flatMap(e => Types.optStr(e, "message"))
            .getOrElse("the subscription failed")
          try sub.onError(BsdkrunError.GraphQL(detail)) catch case _: Exception => ()

      case "complete" =>
        for
          id <- Types.optStr(msg, "id")
          sub <- ws.synchronized(ws.subs.remove(id))
        do try sub.onComplete() catch case _: Exception => ()

      // The daemon pings idle sockets; an unanswered ping closes the stream.
      case "ping" => send(socket, ujson.write(ujson.Obj("type" -> "pong")))

      case _ => ()

  private def listener: WebSocket.Listener = new WebSocket.Listener:
    private val buf = new StringBuilder

    override def onOpen(socket: WebSocket): Unit =
      socket.request(1)
      // `java.net.http.WebSocket` cannot set headers on the handshake, so the
      // token travels in `connection_init` instead — the same reason the
      // browser client does it that way.
      send(
        socket,
        ujson.write(
          ujson.Obj(
            "type" -> "connection_init",
            "payload" -> ujson.Obj("authorization" -> s"Bearer $token")
          )
        )
      )

    override def onText(socket: WebSocket, data: CharSequence, last: Boolean): CompletionStage[?] =
      buf.append(data)
      if last then
        val raw = buf.toString
        buf.setLength(0)
        try handleMessage(socket, raw) catch case _: Exception => ()
      socket.request(1)
      null

    override def onClose(socket: WebSocket, status: Int, reason: String): CompletionStage[?] =
      handleDisconnect()
      null

    override def onError(socket: WebSocket, error: Throwable): Unit =
      handleDisconnect()

  private def ensureSocket(): Either[BsdkrunError, WebSocket] =
    ws.synchronized(ws.socket) match
      case Some(s) => Right(s)
      case None =>
        try
          val built = http
            .newWebSocketBuilder()
            .nn
            .subprotocols("graphql-transport-ws")
            .nn
            .buildAsync(URI.create(wsUrl).nn, listener)
            .nn
            .get()
            .nn
          ws.synchronized:
            ws.socket = Some(built)
            ws.acked = false
            ws.pending = Nil
          Right(built)
        catch
          case e: Exception =>
            val cause = Option(e.getCause).map(_.nn).getOrElse(e)
            Left(BsdkrunError.GraphQL(s"cannot reach the bsdkrun daemon at $wsUrl — ${cause.getMessage}"))

  /** Close the shared websocket and drop every subscription. */
  def close(): Unit = closeSocket()

  // -- typed operations -------------------------------------------------------

  private val MachinesQuery =
    s"query($$all:Boolean!){ machines(all:$$all){ ${Client.MachineFields} } }"

  /** List machines. `all` includes stopped ones (default running only). */
  def listMachines(all: Boolean = false): Either[BsdkrunError, Seq[SandboxInfo]] =
    request(MachinesQuery, ujson.Obj("all" -> all)).map: data =>
      data.objOpt
        .flatMap(_.get("machines"))
        .flatMap(_.arrOpt)
        .map(_.map(Types.sandboxInfo).toSeq)
        .getOrElse(Seq.empty)

  /** A machine by id, name, or unique id prefix. */
  def getMachine(id: String): Either[BsdkrunError, Option[SandboxInfo]] =
    request(
      s"query($$id:String!){ machine(id:$$id){ ${Client.MachineFields} } }",
      ujson.Obj("id" -> id)
    ).map: data =>
      data.objOpt
        .flatMap(_.get("machine"))
        .filter(_ != ujson.Null)
        .map(Types.sandboxInfo)

  private def commandResult(data: Value, field: String, label: String): CommandResult =
    val v = data.objOpt.flatMap(_.get(field)).getOrElse(ujson.Obj())
    CommandResult(
      Types.str(v, "stdout"),
      Types.str(v, "stderr"),
      Types.optInt(v, "exitCode").getOrElse(0),
      label
    )

  def stopMachine(id: String): Either[BsdkrunError, CommandResult] =
    request("mutation($id:String!){ stopMachine(id:$id){ exitCode stdout stderr } }", ujson.Obj("id" -> id))
      .map(commandResult(_, "stopMachine", "stopMachine"))

  def startMachine(id: String): Either[BsdkrunError, CommandResult] =
    request("mutation($id:String!){ startMachine(id:$id){ exitCode stdout stderr } }", ujson.Obj("id" -> id))
      .map(commandResult(_, "startMachine", "startMachine"))

  def removeMachines(ids: Seq[String], force: Boolean = false): Either[BsdkrunError, CommandResult] =
    request(
      "mutation($ids:[String!]!,$force:Boolean!){ removeMachines(ids:$ids, force:$force){ exitCode stdout stderr } }",
      ujson.Obj("ids" -> ids, "force" -> force)
    ).map(commandResult(_, "removeMachines", "removeMachines"))

  def updateMachine(
      id: String,
      cpus: Option[Int] = None,
      mem: Option[Int] = None
  ): Either[BsdkrunError, CommandResult] =
    request(
      "mutation($id:String!,$cpus:Int,$mem:Int){ updateMachine(id:$id, cpus:$cpus, mem:$mem){ exitCode stdout stderr } }",
      ujson.Obj(
        "id" -> id,
        "cpus" -> cpus.map(ujson.Num(_)).getOrElse(ujson.Null),
        "mem" -> mem.map(ujson.Num(_)).getOrElse(ujson.Null)
      )
    ).map(commandResult(_, "updateMachine", "updateMachine"))

  // -- docker -------------------------------------------------------------------
  //
  // bsdkrun runs one `docker:dind` microVM and serves its API on a host unix
  // socket, so these drive the same engine the host's `docker` CLI does.

  /** Is the Docker engine up, and where is its socket? */
  def dockerStatus(): Either[BsdkrunError, DockerStatus] =
    request(s"{ dockerStatus { ${Client.DockerStatusFields} } }")
      .map: data =>
        Types.dockerStatus(data.objOpt.flatMap(_.get("dockerStatus")).getOrElse(ujson.Obj()))

  /** Containers in the engine. `all = false` lists only running ones. */
  def dockerContainers(all: Boolean = true): Either[BsdkrunError, Seq[DockerContainer]] =
    request(
      s"query($$all:Boolean!){ dockerContainers(all:$$all){ ${Client.DockerContainerFields} } }",
      ujson.Obj("all" -> all)
    ).map: data =>
      data.objOpt
        .flatMap(_.get("dockerContainers"))
        .flatMap(_.arrOpt)
        .map(_.map(Types.dockerContainer).toSeq)
        .getOrElse(Seq.empty)

  /** Start (or resume) the engine, returning its status once it answers.
    *
    * Idempotent: the VM has a fixed name, so this resumes the existing one
    * rather than creating a second.
    */
  def dockerStart(
      cpus: Option[Int] = None,
      mem: Option[Int] = None,
      mounts: Seq[String] = Seq.empty,
      noHome: Boolean = false,
      publishBind: Option[String] = None,
      diskSize: Option[String] = None
  ): Either[BsdkrunError, DockerStatus] =
    request(
      s"mutation($$input:DockerStartInput!){ dockerStart(input:$$input){ " +
        s"${Client.DockerStatusFields} } }",
      ujson.Obj(
        "input" -> ujson.Obj(
          "cpus" -> cpus.map(ujson.Num(_)).getOrElse(ujson.Null),
          "mem" -> mem.map(ujson.Num(_)).getOrElse(ujson.Null),
          "mounts" -> mounts,
          "noHome" -> noHome,
          "publishBind" -> publishBind.map(ujson.Str(_)).getOrElse(ujson.Null),
          "diskSize" -> diskSize.map(ujson.Str(_)).getOrElse(ujson.Null)
        )
      )
    ).map: data =>
      Types.dockerStatus(data.objOpt.flatMap(_.get("dockerStart")).getOrElse(ujson.Obj()))

  /** Stop the engine. Images and containers stay on its disk. */
  def dockerStop(): Either[BsdkrunError, CommandResult] =
    request("mutation{ dockerStop{ exitCode stdout stderr } }")
      .map(commandResult(_, "dockerStop", "dockerStop"))

  /** Act on containers: start / stop / restart / kill / pause / unpause / rm. */
  def dockerContainer(action: String, ids: Seq[String]): Either[BsdkrunError, CommandResult] =
    request(
      "mutation($action:String!,$ids:[String!]!){ " +
        "dockerContainer(action:$action, ids:$ids){ exitCode stdout stderr } }",
      ujson.Obj("action" -> action, "ids" -> ids)
    ).map(commandResult(_, "dockerContainer", "dockerContainer"))

  /** One container's logs (stdout+stderr, most recent `tail` lines). */
  def dockerLogs(id: String, tail: Int = 200): Either[BsdkrunError, String] =
    request(
      "query($id:String!,$tail:Int!){ dockerContainerLogs(id:$id, tail:$tail) }",
      ujson.Obj("id" -> id, "tail" -> tail)
    ).map(data => Types.str(data, "dockerContainerLogs"))

  // -- snapshots --------------------------------------------------------------
  //
  // A snapshot is a copy-on-write clone of a machine's disk state: instant to
  // take, free until the two sides diverge. `branch` boots a new machine from
  // one; `restoreMachine`/`rollbackMachine` put one back.

  /** Snapshots, newest first. `machine` narrows the list to one machine's. */
  def snapshots(machine: Option[String] = None): Either[BsdkrunError, Seq[SnapshotInfo]] =
    request(
      s"query($$machine:String){ snapshots(machine:$$machine){ ${Client.SnapshotFields} } }",
      ujson.Obj("machine" -> machine.map(ujson.Str(_)).getOrElse(ujson.Null))
    ).map: data =>
      data.objOpt
        .flatMap(_.get("snapshots"))
        .flatMap(_.arrOpt)
        .map(_.map(Types.snapshotInfo).toSeq)
        .getOrElse(Seq.empty)

  /** Capture a machine's disk state. `name` of `None` yields `<machine>-<n>`.
    *
    * A BSD guest is powered off first — a mounted UFS cannot be cloned
    * consistently — so the machine is left stopped; `startMachine` brings it
    * back.
    */
  def snapshotMachine(
      id: String,
      name: Option[String] = None,
      description: String = ""
  ): Either[BsdkrunError, SnapshotInfo] =
    request(
      s"mutation($$id:String!,$$name:String,$$description:String!){ " +
        s"snapshotMachine(id:$$id, name:$$name, description:$$description){ ${Client.SnapshotFields} } }",
      ujson.Obj(
        "id" -> id,
        "name" -> name.map(ujson.Str(_)).getOrElse(ujson.Null),
        "description" -> description
      )
    ).map: data =>
      Types.snapshotInfo(data.objOpt.flatMap(_.get("snapshotMachine")).getOrElse(ujson.Obj()))

  /** Delete snapshots and their data. Machines branched from them are
    * unaffected.
    */
  def removeSnapshots(names: Seq[String]): Either[BsdkrunError, CommandResult] =
    request(
      "mutation($names:[String!]!){ removeSnapshots(names:$names){ exitCode stdout stderr } }",
      ujson.Obj("names" -> names)
    ).map(commandResult(_, "removeSnapshots", "removeSnapshots"))

  /** Put a machine's disk state back to one of its snapshots.
    *
    * `force` stops the machine first — it holds the very files being replaced.
    * `backup` snapshots the state being overwritten, which is a CoW clone and
    * therefore free. The machine is left stopped.
    */
  def restoreMachine(
      id: String,
      snapshot: String,
      force: Boolean = true,
      backup: Boolean = true
  ): Either[BsdkrunError, CommandResult] =
    request(
      "mutation($id:String!,$snapshot:String!,$force:Boolean!,$backup:Boolean!){ " +
        "restoreMachine(id:$id, snapshot:$snapshot, force:$force, backup:$backup){ exitCode stdout stderr } }",
      ujson.Obj("id" -> id, "snapshot" -> snapshot, "force" -> force, "backup" -> backup)
    ).map(commandResult(_, "restoreMachine", "restoreMachine"))

  /** Restore a machine to its most recent snapshot. */
  def rollbackMachine(
      id: String,
      force: Boolean = true,
      backup: Boolean = true
  ): Either[BsdkrunError, CommandResult] =
    request(
      "mutation($id:String!,$force:Boolean!,$backup:Boolean!){ " +
        "rollbackMachine(id:$id, force:$force, backup:$backup){ exitCode stdout stderr } }",
      ujson.Obj("id" -> id, "force" -> force, "backup" -> backup)
    ).map(commandResult(_, "rollbackMachine", "rollbackMachine"))

  /** Boot a NEW machine from a snapshot — or from a machine, which is
    * snapshotted first — and return the new machine's id.
    *
    * The state is cloned, never booted in place, so the source is untouched
    * and one snapshot can be branched any number of times. Empty `ports`
    * inherits the snapshot's own forwards, with any host port that is already
    * taken swapped for a free one; `noPorts` drops them instead.
    */
  def branch(
      snapshot: String,
      name: Option[String] = None,
      cpus: Option[Int] = None,
      mem: Option[Int] = None,
      ports: Seq[String] = Seq.empty,
      noPorts: Boolean = false
  ): Either[BsdkrunError, String] =
    request(
      "mutation($input:BranchInput!){ branchSnapshot(input:$input) }",
      ujson.Obj(
        "input" -> ujson.Obj(
          "snapshot" -> snapshot,
          "name" -> name.map(ujson.Str(_)).getOrElse(ujson.Null),
          "cpus" -> cpus.map(ujson.Num(_)).getOrElse(ujson.Null),
          "mem" -> mem.map(ujson.Num(_)).getOrElse(ujson.Null),
          "ports" -> ports,
          "noPorts" -> noPorts
        )
      )
    ).map(data => Types.str(data, "branchSnapshot"))

  /** Read a machine's console log, or bsdkrun's own boot log. */
  def logs(id: String, boot: Boolean = false): Either[BsdkrunError, String] =
    request(
      "query($id:String!,$boot:Boolean!){ machineLogs(id:$id, boot:$boot) }",
      ujson.Obj("id" -> id, "boot" -> boot)
    ).map(data => Types.str(data, "machineLogs"))

  /** Stream a machine's console log. Returns a function that stops the stream. */
  def followLogs(
      id: String,
      boot: Boolean = false,
      onLine: String => Unit
  ): Either[BsdkrunError, () => Unit] =
    subscribe(
      "subscription($id:String!,$follow:Boolean!,$boot:Boolean!){ machineLogStream(id:$id, follow:$follow, boot:$boot) }",
      ujson.Obj("id" -> id, "follow" -> true, "boot" -> boot),
      data => onLine(Types.str(data, "machineLogStream"))
    )

  // -- remote exec / shell ----------------------------------------------------

  private def envList(env: Map[String, String]): Seq[String] =
    env.toSeq.sortBy(_._1).map((k, v) => s"$k=$v")

  private def openShell(
      id: String,
      command: Seq[String],
      env: Map[String, String],
      rows: Int,
      cols: Int
  ): Either[BsdkrunError, String] =
    request(
      "mutation($m:String!,$c:[String!]!,$e:[String!]!,$r:Int!,$k:Int!){ " +
        "openShell(machineId:$m, command:$c, env:$e, rows:$r, cols:$k){ id } }",
      ujson.Obj("m" -> id, "c" -> command, "e" -> envList(env), "r" -> rows, "k" -> cols)
    ).flatMap: data =>
      data.objOpt
        .flatMap(_.get("openShell"))
        .flatMap(v => Types.optStr(v, "id"))
        .toRight(BsdkrunError.GraphQL("openShell returned no session id"))

  /** Idempotent server-side. A failure here — session already gone, machine
    * removed — must never mask the caller's real result, so it is swallowed.
    */
  private def closeShell(sessionId: String): Unit =
    val _ = request("mutation($s:String!){ closeShell(sessionId:$s) }", ujson.Obj("s" -> sessionId))

  /** Run a command on a remote machine to completion and collect its output.
    *
    * The three-operation sequence the daemon documents: `openShell` with a
    * command (so the session runs it rather than a login shell), **then**
    * subscribe to `shellOutput` so nothing written in between is lost, then
    * wait for an exit code. `closeShell` always runs.
    */
  def exec(
      id: String,
      command: Seq[String],
      env: Map[String, String] = Map.empty,
      timeout: Duration = Duration.Inf
  ): Either[BsdkrunError, CommandResult] =
    openShell(id, command, env, 24, 80).flatMap: sessionId =>
      val out = new ByteArrayOutputStream()
      val result = new java.util.concurrent.LinkedBlockingQueue[Either[BsdkrunError, Option[Int]]](1)

      val subscribed = subscribe(
        "subscription($s:String!){ shellOutput(sessionId:$s){ dataBase64 exitCode } }",
        ujson.Obj("s" -> sessionId),
        data =>
          data.objOpt.flatMap(_.get("shellOutput")).foreach: payload =>
            Types.optStr(payload, "dataBase64").foreach(b64 => out.write(Base64.getDecoder.nn.decode(b64).nn))
            Types.optInt(payload, "exitCode").foreach(code => { val _ = result.offer(Right(Some(code))) }),
        err => { val _ = result.offer(Left(err)) },
        () => { val _ = result.offer(Right(None)) }
      )

      val outcome = subscribed.flatMap: unsubscribe =>
        try
          val taken =
            if timeout.isFinite then Option(result.poll(timeout.toMillis, java.util.concurrent.TimeUnit.MILLISECONDS))
            else Some(result.take())
          taken match
            case None                 => Left(BsdkrunError.GraphQL(s"timed out waiting for ${command.mkString(" ")}"))
            case Some(Left(err))      => Left(err)
            case Some(Right(exitCode)) =>
              Right(
                CommandResult(
                  new String(out.toByteArray.nn, UTF_8),
                  "",
                  exitCode.getOrElse(0),
                  s"exec ${command.mkString(" ")}"
                )
              )
        finally unsubscribe()

      closeShell(sessionId)
      outcome

  /** A live interactive session on a remote machine. */
  final class ShellSession private[Client] (
      val id: String,
      unsubscribe: () => Unit,
      buffered: mutable.Queue[Either[Int, Array[Byte]]]
  ):
    private var outputHandler: Option[Array[Byte] => Unit] = None
    private var exitHandler: Option[Int => Unit] = None

    /** Register the output callback.
      *
      * Anything that arrived before this was called is replayed now rather
      * than dropped: the subscription necessarily starts before the caller can
      * register a handler, and that race has bitten other SDKs.
      */
    def onOutput(f: Array[Byte] => Unit): ShellSession = synchronized:
      outputHandler = Some(f)
      drain()
      this

    def onExit(f: Int => Unit): ShellSession = synchronized:
      exitHandler = Some(f)
      drain()
      this

    private[Client] def push(event: Either[Int, Array[Byte]]): Unit = synchronized:
      buffered.enqueue(event)
      drain()

    private def drain(): Unit =
      while buffered.nonEmpty && buffered.head.fold(_ => exitHandler.isDefined, _ => outputHandler.isDefined) do
        buffered.dequeue() match
          case Right(bytes) => outputHandler.foreach(f => try f(bytes) catch case _: Exception => ())
          case Left(code)   => exitHandler.foreach(f => try f(code) catch case _: Exception => ())

    /** Write to the session's stdin. */
    def write(data: Array[Byte]): Either[BsdkrunError, Unit] =
      request(
        "mutation($s:String!,$d:String!){ writeShell(sessionId:$s, dataBase64:$d) }",
        ujson.Obj("s" -> id, "d" -> Base64.getEncoder.nn.encodeToString(data).nn)
      ).map(_ => ())

    def write(text: String): Either[BsdkrunError, Unit] = write(text.getBytes(UTF_8).nn)

    /** Tell the guest the terminal was resized. */
    def resize(rows: Int, cols: Int): Either[BsdkrunError, Unit] =
      request(
        "mutation($s:String!,$r:Int!,$k:Int!){ resizeShell(sessionId:$s, rows:$r, cols:$k) }",
        ujson.Obj("s" -> id, "r" -> rows, "k" -> cols)
      ).map(_ => ())

    def close(): Unit =
      unsubscribe()
      closeShell(id)

  /** Open a live interactive session. Returns immediately with a handle whose
    * callbacks fire as output arrives.
    */
  def shell(
      id: String,
      command: Seq[String] = Seq.empty,
      env: Map[String, String] = Map.empty,
      rows: Int = 24,
      cols: Int = 80
  ): Either[BsdkrunError, ShellSession] =
    openShell(id, command, env, rows, cols).flatMap: sessionId =>
      val buffered = mutable.Queue.empty[Either[Int, Array[Byte]]]
      var session: ShellSession = null

      subscribe(
        "subscription($s:String!){ shellOutput(sessionId:$s){ dataBase64 exitCode } }",
        ujson.Obj("s" -> sessionId),
        data =>
          data.objOpt.flatMap(_.get("shellOutput")).foreach: payload =>
            Types.optStr(payload, "dataBase64").foreach(b64 =>
              session.push(Right(Base64.getDecoder.nn.decode(b64).nn))
            )
            Types.optInt(payload, "exitCode").foreach(code => session.push(Left(code)))
      ).map: unsubscribe =>
        session = new ShellSession(sessionId, unsubscribe, buffered)
        session

object Client:

  val UrlEnv = "BSDKRUN_URL"
  val TokenEnv = "BSDKRUN_TOKEN"

  /** The `Machine` field selection shared by the machine queries. */
  private[bsdkrun] val MachineFields =
    "id name image kind command status running exitCode pid detached " +
      "cpus mem volume stateDir createdAt finishedAt network netIp origin " +
      "ports { bind host guest }"

  private[bsdkrun] val DockerStatusFields =
    "running machineId machineRunning socket socketReady apiPort version " +
      "containers images mounts disk diskSize"

  private[bsdkrun] val DockerContainerFields =
    "id name image command state status ports created"

  private[bsdkrun] val SnapshotFields =
    "id name machineId machineName kind image path parent description " +
      "cpus mem size createdAt ports { bind host guest }"

  /** Trim, add `http://` if no scheme was given, strip trailing slashes, and
    * append `/graphql` unless the path already ends with it.
    */
  def normalizeUrl(input: String): String =
    val s = input.trim
    if s.isEmpty then s
    else
      val withScheme = if s.matches("(?i)^https?://.*") then s else s"http://$s"
      val trimmed = withScheme.replaceAll("/+$", "")
      if trimmed.matches("(?i).*/graphql$") then trimmed else s"$trimmed/graphql"

  /** Derive the websocket endpoint from the HTTP one. */
  def wsUrl(httpUrl: String): String =
    val scheme = httpUrl
      .replaceFirst("(?i)^https://", "wss://")
      .replaceFirst("(?i)^http://", "ws://")
    scheme.replaceAll("/+$", "") + "/ws"

  /** Build a client from an explicit URL and token. */
  def apply(url: String, token: String): Client = new Client(normalizeUrl(url), token)

  /** Build a client from `BSDKRUN_URL` / `BSDKRUN_TOKEN`.
    *
    * A URL set without a token is an error rather than a silent
    * unauthenticated fallback. Takes an explicit env map so tests need not
    * mutate real process state, which the JVM offers no portable way to do.
    */
  def fromEnv(env: Map[String, String]): Either[BsdkrunError, Client] =
    env.get(UrlEnv).filter(_.trim.nonEmpty) match
      case None =>
        Left(BsdkrunError.MissingConfig(s"$UrlEnv is not set; nothing to connect to"))
      case Some(url) =>
        env.get(TokenEnv).filter(_.trim.nonEmpty) match
          case None        => Left(BsdkrunError.MissingConfig(s"$UrlEnv is set but $TokenEnv is not"))
          case Some(token) => Right(Client(url, token))

  def fromEnv(): Either[BsdkrunError, Client] =
    fromEnv(
      Seq(UrlEnv, TokenEnv).flatMap(k => Option(java.lang.System.getenv(k)).map(k -> _.nn)).toMap
    )

  /** Pull `data` out of a GraphQL response, turning `errors` into the right
    * failure — an `UNAUTHENTICATED` code is an auth problem, not a generic one.
    */
  private[bsdkrun] def dataOrError(json: Value): Either[BsdkrunError, Value] =
    val errors = json.objOpt.flatMap(_.get("errors")).flatMap(_.arrOpt).filter(_.nonEmpty)
    errors match
      case Some(list) =>
        val first = list.head
        val message = Types.optStr(first, "message").getOrElse("the daemon returned an error")
        val code = first.objOpt
          .flatMap(_.get("extensions"))
          .flatMap(e => Types.optStr(e, "code"))
        if code.contains("UNAUTHENTICATED") then Left(BsdkrunError.Auth(message))
        else Left(BsdkrunError.GraphQL(message, code))
      case None =>
        json.objOpt
          .flatMap(_.get("data"))
          .toRight(BsdkrunError.GraphQL("the daemon returned no data"))
