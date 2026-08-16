package bsdkrun

import java.nio.file.{Files, Path}

class BinarySuite extends munit.FunSuite:

  private def env(
      override_ : Option[String] = None,
      bin: Option[String] = None,
      path: Option[String] = None,
      repo: Option[Path] = None
  ) = Binary.Env(override_, bin, path, repo)

  test("candidates are ordered override, BSDKRUN_BIN, PATH, then the dev build") {
    val repo = Files.createTempDirectory("bsdkrun-repo").nn
    val got = Binary.candidates(
      env(override_ = Some("/o/bsdkrun"), bin = Some("/e/bsdkrun"), repo = Some(repo))
    )
    assertEquals(
      got,
      Seq(
        "/o/bsdkrun",
        "/e/bsdkrun",
        repo.resolve("target/release/bsdkrun").nn.toString,
        repo.resolve("target/debug/bsdkrun").nn.toString
      )
    )
  }

  test("an executable on PATH is found, a non-executable file is not") {
    val dir = Files.createTempDirectory("bsdkrun-path").nn
    val exe = dir.resolve("bsdkrun").nn
    Files.write(exe, "#!/bin/sh\n".getBytes)
    // Not executable yet: discovery must skip it rather than pick a file it
    // cannot run.
    assert(!Binary.candidates(env(path = Some(dir.toString))).contains(exe.toString))

    exe.toFile.nn.setExecutable(true)
    assert(Binary.candidates(env(path = Some(dir.toString))).contains(exe.toString))
  }

  test("resolving with nothing available names everything it searched") {
    val err = Binary
      .resolveWith(env(override_ = Some("/nope/bsdkrun"), path = Some("")))
      .swap
      .getOrElse(fail("expected an error"))
    assert(err.message.contains("/nope/bsdkrun"), err.message)
    assert(err.message.contains("BSDKRUN_BIN"), err.message)
  }

class ShellSuite extends munit.FunSuite:
  import Shell.sh

  test("interpolated values are single-quoted") {
    assertEquals(sh"echo ${"hello"}", "echo 'hello'")
  }

  // The point of the quoting: an interpolated value is data, never syntax.
  test("injection attempts stay quoted") {
    assertEquals(sh"echo ${"; rm -rf /"}", "echo '; rm -rf /'")
    assertEquals(sh"echo ${"$(whoami)"}", "echo '$(whoami)'")
    assertEquals(sh"echo ${"it's"}", """echo 'it'\''s'""")
  }

  test("raw opts a fragment out of quoting") {
    assertEquals(sh"ls ${Shell.raw("-la")} ${"/tmp"}", "ls -la '/tmp'")
  }

  test("a collection interpolates as separate quoted words") {
    assertEquals(sh"cat ${Seq("a b", "c")}", "cat 'a b' 'c'")
  }

class CommandResultSuite extends munit.FunSuite:

  test("text trims and lines drops blanks") {
    val r = CommandResult("a\n\nb\n", "", 0, "x")
    assertEquals(r.text, "a\n\nb")
    assertEquals(r.lines, Seq("a", "b"))
  }

  test("checked turns a non-zero exit into a Left") {
    assert(CommandResult("", "", 0, "x").checked.isRight)
    val err = CommandResult("", "boom", 3, "uname").checked.swap.getOrElse(fail("expected an error"))
    assert(err.message.contains("exit 3"), err.message)
    assert(err.message.contains("boom"), err.message)
  }

class TypesSuite extends munit.FunSuite:

  test("a ps row decodes, including its ports") {
    val row = ujson.read(
      """{"id":"abc123","name":"web","image":"alpine","kind":"linux","command":"sh",
         "running":true,"exit_code":null,"pid":42,"detached":true,"cpus":2,"mem":1024,
         "volume":null,"state_dir":"/s","network":"devnet","net_ip":"192.168.127.2",
         "ports":[{"bind":"127.0.0.1","host":8080,"guest":80}],
         "created_at":1700000000,"finished_at":null}"""
    )
    val info = Types.sandboxInfo(row)
    assertEquals(info.id, "abc123")
    assertEquals(info.name, Some("web"))
    assertEquals(info.running, true)
    assertEquals(info.status, "running")
    assertEquals(info.exitCode, None)
    assertEquals(info.pid, Some(42))
    assertEquals(info.stateDir, "/s")
    assertEquals(info.ports, Seq(Types.PortForward("127.0.0.1", 8080, 80)))
    assertEquals(info.createdAt, Some(1700000000L))
  }

  // GraphQL has no 64-bit integer type, so the daemon sends timestamps as
  // strings and the field names arrive camelCase. The same decoder has to read
  // both shapes or the remote client would need a second one.
  test("the same decoder reads the daemon's camelCase and string numbers") {
    val row = ujson.read(
      """{"id":"abc","image":"alpine","kind":"linux","running":false,
         "exitCode":143,"stateDir":"/s","netIp":"10.0.0.2","createdAt":"1700000000"}"""
    )
    val info = Types.sandboxInfo(row)
    assertEquals(info.exitCode, Some(143))
    assertEquals(info.stateDir, "/s")
    assertEquals(info.netIp, Some("10.0.0.2"))
    assertEquals(info.createdAt, Some(1700000000L))
    assertEquals(info.status, "exited")
  }

  test("a missing field is a default, not a failure") {
    val info = Types.sandboxInfo(ujson.read("""{"id":"x"}"""))
    assertEquals(info.id, "x")
    assertEquals(info.name, None)
    assertEquals(info.cpus, 0)
    assertEquals(info.ports, Seq.empty)
  }

  test("rows and one report the raw text when the JSON is wrong") {
    assert(Types.rows("not json", "label", Types.sandboxInfo).isLeft)
    assertEquals(Types.rows("", "label", Types.sandboxInfo), Right(Seq.empty))
    assert(Types.one("[]", "label").isLeft)
    assert(Types.one("", "label").isRight)
  }

class CacheDecodeSuite extends munit.FunSuite:

  test("a restore result decodes a hit, a miss, and a fallback") {
    val hit = RestoreResult.decode(
      ujson.read("""{"restored":true,"requested_key":"k","key":"k","path":"/p","size":9,
                     "compression":"zstd","created":1700000000}""")
    )
    assertEquals(hit.restored, true)
    assertEquals(hit.key, Some("k"))
    assertEquals(hit.size, Some(9L))

    val miss = RestoreResult.decode(ujson.read("""{"restored":false,"requested_key":"k"}"""))
    assertEquals(miss.restored, false)
    assertEquals(miss.key, None)

    val fallback = RestoreResult.decode(
      ujson.read("""{"restored":true,"requested_key":"asked","key":"landed"}""")
    )
    assertEquals(fallback.requestedKey, "asked")
    assertEquals(fallback.key, Some("landed"))
  }

  test("compression flags match the CLI's spelling") {
    assertEquals(Compression.values.map(_.flag).toSeq, Seq("gzip", "zstd", "estargz", "none"))
  }
