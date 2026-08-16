package bsdkrun

import bsdkrun.Shell.sh

/** End-to-end tests: these boot a real Alpine microVM through the `bsdkrun`
  * binary, so they need libkrun + KVM (Linux) or Hypervisor.framework (macOS).
  *
  * They run only when `BSDKRUN_E2E=1` is set — CI sets it after building the
  * binary — so a plain `sbt test` on a machine without the toolchain runs the
  * unit suites and skips these.
  */
class E2ESuite extends munit.FunSuite:

  private val enabled = sys.env.get("BSDKRUN_E2E").contains("1")
  private val image = sys.env.getOrElse("BSDKRUN_E2E_IMAGE", "alpine")

  override def munitIgnore: Boolean = !enabled

  // One machine for the whole suite: booting is the slow part, and every
  // assertion below is independent of the others.
  private var sbx: Sandbox = null

  override def beforeAll(): Unit =
    if enabled then
      sbx = Sandbox
        .linux(image)
        .name(s"scala-e2e-${System.nanoTime() % 100000}")
        .env("GREETING", "hello-from-scala")
        .command("sleep", "600")
        .create()
        .fold(e => throw BsdkrunException(e), identity)
      waitForAgent()

  override def afterAll(): Unit =
    if enabled && sbx != null then
      val _ = sbx.remove(force = true)

  /** The agent comes up a moment after the machine does; poll rather than
    * sleeping a fixed amount, which is either flaky or wasteful.
    */
  private def waitForAgent(): Unit =
    val deadline = System.currentTimeMillis() + 120_000
    var ready = false
    while !ready && System.currentTimeMillis() < deadline do
      ready = sbx.exec("true").exists(_.ok)
      if !ready then Thread.sleep(2000)
    if !ready then fail("the guest agent never became reachable")

  test("exec runs a command and collects its output") {
    val res = sbx.exec("uname", "-s").fold(e => fail(e.message), identity)
    assertEquals(res.exitCode, 0)
    assertEquals(res.text, "Linux")
  }

  test("a failing command reports its exit code rather than throwing") {
    val res = sbx.exec("sh", "-c", "exit 3").fold(e => fail(e.message), identity)
    assertEquals(res.exitCode, 3)
    assert(res.checked.isLeft)
  }

  test("sh runs a script, and the interpolator quotes its arguments") {
    // The value contains shell metacharacters; the interpolator has to make it
    // an argument rather than syntax, so `echo` prints it back verbatim.
    val value = "hello; rm -rf /"
    val res = sbx.sh(sh"echo $value").fold(e => fail(e.message), identity)
    assertEquals(res.text, value)
  }

  test("exec env and cwd reach the command") {
    val res = sbx
      .exec(Seq("sh", "-c", "echo $FOO:$(pwd)"), ExecOptions(env = Map("FOO" -> "bar"), cwd = Some("/tmp")))
      .fold(e => fail(e.message), identity)
    assertEquals(res.text, "bar:/tmp")
  }

  // Boot-time env has to reach the *workload*, not just `exec` — reading it out
  // of exec's own environment would prove nothing about `-e` at create.
  test("create-time env reaches the machine's workload") {
    val res = sbx
      .sh(
        "for p in /proc/[0-9]*; do grep -qa '^sleep' $p/cmdline 2>/dev/null && " +
          "tr '\\0' '\\n' < $p/environ | grep '^GREETING=' && break; done"
      )
      .fold(e => fail(e.message), identity)
    assertEquals(res.text, "GREETING=hello-from-scala")
  }

  test("files round-trip through the guest, text and binary alike") {
    val text = "written by the scala sdk\n"
    sbx.fs.writeFile("/app/main.txt", text).fold(e => fail(e.message), identity)
    assertEquals(sbx.fs.readText("/app/main.txt").fold(e => fail(e.message), identity), text)

    // Bytes, because a UTF-8 round trip would silently mangle these.
    val blob = Array[Byte](0, 1, 2, -1, -2, 0, -128)
    sbx.fs.writeFile("/app/blob.bin", blob).fold(e => fail(e.message), identity)
    val back = sbx.fs.readFile("/app/blob.bin").fold(e => fail(e.message), identity)
    assert(back.sameElements(blob), s"binary round-trip corrupted: ${back.toSeq}")
  }

  test("reading a file that is not there is a typed failure") {
    sbx.fs.readText("/definitely/not/here") match
      case Left(BsdkrunError.FileTransfer(path, detail)) =>
        assertEquals(path, "/definitely/not/here")
        assert(detail.nonEmpty, detail)
      case other => fail(s"expected a FileTransfer error, got $other")
  }

  test("a directory uploads and comes back with its contents intact") {
    val dir = java.nio.file.Files.createTempDirectory("scala-e2e").nn
    java.nio.file.Files.write(dir.resolve("a.txt").nn, "alpha\n".getBytes)
    java.nio.file.Files.createDirectories(dir.resolve("nested").nn)
    java.nio.file.Files.write(dir.resolve("nested/b.txt").nn, "beta\n".getBytes)

    sbx.fs.upload(dir.toString, "/app/tree").fold(e => fail(e.message), identity)
    val listing = sbx.exec("find", "/app/tree", "-type", "f").fold(e => fail(e.message), identity)
    assertEquals(listing.lines.sorted, Seq("/app/tree/a.txt", "/app/tree/nested/b.txt"))
  }

  test("a cache entry saves, restores, and reports a miss without failing") {
    val key = s"scala-e2e-${System.nanoTime() % 100000}"
    try
      val entry = sbx.cache
        .save("/app", key, Compression.Zstd, force = true)
        .fold(e => fail(e.message), identity)
      assertEquals(entry.key, key)
      assert(entry.size > 0, s"empty archive: $entry")

      val hit = sbx.cache.restore(key, Some("/restored")).fold(e => fail(e.message), identity)
      assertEquals(hit.restored, true)
      assertEquals(hit.key, Some(key))

      // A miss is an ordinary answer — a first CI run depends on it.
      val miss = sbx.cache.restore(s"$key-absent").fold(e => fail(e.message), identity)
      assertEquals(miss.restored, false)
      assertEquals(miss.key, None)

      // An exact miss falls back to a prefix, landing on the entry above.
      val fallback = sbx.cache
        .restore(s"$key-nope", Some("/fallback"), Seq(key))
        .fold(e => fail(e.message), identity)
      assertEquals(fallback.restored, true)
      assertEquals(fallback.key, Some(key))
    finally
      val _ = Cache.remove(Seq(key))
  }

  test("the machine shows up in the listing and can be fetched by id") {
    val rows = Sandbox.list(all = true).fold(e => fail(e.message), identity)
    assert(rows.exists(_.id == sbx.id), s"${sbx.id} missing from ${rows.map(_.id)}")

    val again = Sandbox.get(sbx.id).fold(e => fail(e.message), identity)
    assertEquals(again.id, sbx.id)
    assertEquals(sbx.isRunning().fold(e => fail(e.message), identity), true)
  }

  test("an unknown id is a SandboxNotFound, not something vaguer") {
    Sandbox.get("definitely-not-a-machine") match
      case Left(BsdkrunError.SandboxNotFound(id)) => assertEquals(id, "definitely-not-a-machine")
      case other                                  => fail(s"expected SandboxNotFound, got $other")
  }

  test("the host-level namespaces answer") {
    assert(Images.list().isRight)
    assert(Volumes.list().isRight)
    assert(Host.probe(), "bsdkrun probe failed — the host cannot create a VM context")
  }
