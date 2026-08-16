package bsdkrun

import bsdkrun.Args.{CreateOptions, NetOptions, Os}

class ArgsSuite extends munit.FunSuite:

  private def build(o: CreateOptions): Seq[String] =
    Args.build(o).fold(e => fail(e.message), identity)

  test("linux minimal") {
    assertEquals(build(CreateOptions(Os.Linux, image = Some("alpine"))), Seq("linux", "alpine", "-d"))
  }

  test("linux full") {
    val argv = build(
      CreateOptions(
        Os.Linux,
        image = Some("ghcr.io/owner/name:tag"),
        kernel = Some("vmlinux"),
        kernelVersion = Some("6.6"),
        initramfs = true,
        volume = Some("web"),
        mounts = Seq("~/project:/src", "~/data:/data:ro"),
        entrypoint = Some("/bin/sh"),
        console = Some("hvc0"),
        net = NetOptions(ports = Seq("8080:80", "2222:22"), network = Some("devnet")),
        name = Some("api"),
        cpus = Some(2),
        mem = Some(1024),
        command = Seq("node", "server.js")
      )
    )
    assertEquals(
      argv,
      Seq(
        "linux", "ghcr.io/owner/name:tag", "-d",
        "--kernel", "vmlinux",
        "--kernel-version", "6.6",
        "--initramfs",
        "-v", "web",
        "--mount", "~/project:/src",
        "--mount", "~/data:/data:ro",
        "--entrypoint", "/bin/sh",
        "--console", "hvc0",
        "--port", "8080:80",
        "--port", "2222:22",
        "--network", "devnet",
        "--name", "api",
        "--cpus", "2",
        "--mem", "1024",
        "--", "node", "server.js"
      )
    )
  }

  // A caller adds variables in whatever order suits them, so the builder sorts
  // by key — otherwise the same options would produce a different command line
  // run to run, and this assertion could not exist.
  test("linux env is emitted sorted by key") {
    val argv = build(
      CreateOptions(Os.Linux, image = Some("alpine"), env = Seq("ZED" -> "3", "ALPHA" -> "1", "MID" -> "2"))
    )
    assertEquals(argv, Seq("linux", "alpine", "-d", "-e", "ALPHA=1", "-e", "MID=2", "-e", "ZED=3"))
  }

  test("linux without env emits nothing") {
    assertEquals(build(CreateOptions(Os.Linux, image = Some("alpine"))), Seq("linux", "alpine", "-d"))
  }

  test("freebsd and netbsd take disk-persistence flags") {
    assertEquals(
      build(CreateOptions(Os.Freebsd, version = Some("15.1"), persist = true, attachDisk = Seq("extra.raw"))),
      Seq("freebsd", "-d", "--version", "15.1", "--persist", "--attach-disk", "extra.raw")
    )
    assertEquals(
      build(CreateOptions(Os.Netbsd, force = true, volume = Some("nb"))),
      Seq("netbsd", "-d", "--force", "-v", "nb")
    )
  }

  test("firmware and kernel put their required paths up front") {
    assertEquals(
      build(CreateOptions(Os.Firmware, firmware = Some("KRUN_EFI.fd"), disk = Some("d.raw"))),
      Seq("firmware", "--firmware", "KRUN_EFI.fd", "--disk", "d.raw", "-d")
    )
    assertEquals(
      build(CreateOptions(Os.Kernel, kernel = Some("vmlinux"), cmdline = Some("console=ttyS0"))),
      Seq("kernel", "--kernel", "vmlinux", "-d", "--cmdline", "console=ttyS0")
    )
  }

  // The unikernels take their image/path *last*, after the flags — get that
  // wrong and the CLI reads the path as a flag's value.
  test("unikernel kinds put the image or path last") {
    assertEquals(
      build(CreateOptions(Os.Unikraft, path = Some("./app"), cmdline = Some("-p 8080"))),
      Seq("unikraft", "-d", "--cmdline", "-p 8080", "./app")
    )
    assertEquals(build(CreateOptions(Os.Unikraft)), Seq("unikraft", "-d", "."))
    assertEquals(
      build(CreateOptions(Os.Nanos, image = Some("app.img"), persist = true)),
      Seq("nanos", "-d", "--persist", "app.img")
    )
    assertEquals(
      build(CreateOptions(Os.Osv, image = Some("loader.img"), gic = Some("v3"))),
      Seq("osv", "-d", "--gic", "v3", "loader.img")
    )
  }

  // MirageOS options look like bsdkrun's own (`--ipv4=...`), so the CLI takes
  // them as trailing args behind a literal `--`.
  test("solo5 puts guest args behind a -- separator") {
    assertEquals(
      build(
        CreateOptions(
          Os.Solo5,
          path = Some("./dist/app.hvt"),
          block = Seq("storage=disk.img"),
          guestArgs = Seq("--ipv4=10.0.0.2/24")
        )
      ),
      Seq("solo5", "-d", "--block", "storage=disk.img", "./dist/app.hvt", "--", "--ipv4=10.0.0.2/24")
    )
  }

  test("a unikraft guest gets no disk flags even when they are set") {
    // A unikernel has no disk to persist or attach; the CLI would reject them.
    val argv = build(CreateOptions(Os.Unikraft, persist = true, attachDisk = Seq("x.raw")))
    assert(!argv.contains("--persist"), argv)
    assert(!argv.contains("--attach-disk"), argv)
  }

  test("a missing required field is reported, not silently dropped") {
    val err = Args.build(CreateOptions(Os.Linux)).swap.getOrElse(fail("expected an error"))
    assert(err.message.contains("image"), err.message)

    val kernelErr = Args.build(CreateOptions(Os.Kernel)).swap.getOrElse(fail("expected an error"))
    assert(kernelErr.message.contains("kernel"), kernelErr.message)
  }

  test("Os parses the CLI's spelling and rejects anything else") {
    assertEquals(Os.fromString("linux"), Right(Os.Linux))
    assertEquals(Os.fromString("FreeBSD"), Right(Os.Freebsd))
    val err = Os.fromString("plan9").swap.getOrElse(fail("expected an error"))
    assert(err.message.contains("plan9"), err.message)
    assert(err.message.contains("linux"), err.message)
  }

  test("no-net and mac reach the argv") {
    assertEquals(
      build(CreateOptions(Os.Linux, image = Some("alpine"), net = NetOptions(disabled = true, mac = Some("aa:bb:cc:dd:ee:ff")))),
      Seq("linux", "alpine", "-d", "--no-net", "--mac", "aa:bb:cc:dd:ee:ff")
    )
  }
