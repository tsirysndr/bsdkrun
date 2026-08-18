package bsdkrun

/** The YAML the builder emits is consumed by tangled's own workflow parser
  * (inside `bsdkrun ci`), so these tests pin the emitted shape — a change
  * here is a change to what spindle would receive.
  */
class CISuite extends munit.FunSuite:

  test("renders the full workflow shape") {
    val y = CI
      .workflow("test")
      .onPush("main")
      .onPullRequest("main", "develop")
      .deps("scala", "sbt")
      .depsFrom("github:nix-community/fenix/abc123", "stable.default")
      .env("SBT_OPTS", "-Xmx1g")
      .withCloneDepth(100)
      .step("compile", "sbt compile")
      .step("test", "sbt test", Map("CI_STRICT" -> "1"))
      .yaml

    assert(y.contains("  - event: [\"push\"]\n    branch: \"main\""))
    assert(y.contains("branch: [\"main\", \"develop\"]"))
    assert(y.contains("engine: nixery"))
    assert(y.contains("\"nixpkgs\":\n    - \"scala\"\n    - \"sbt\""))
    assert(y.contains("\"github:nix-community/fenix/abc123\":"))
    assert(y.contains("SBT_OPTS: \"-Xmx1g\""))
    assert(y.contains("depth: 100"))
    assert(y.contains("- name: \"compile\"\n    command: |\n      sbt compile"))
    assert(y.contains("environment:\n      CI_STRICT: \"1\""))
  }

  test("block-unsafe commands fall back to a JSON string") {
    // Trailing spaces do not survive a literal block scalar; the emitter
    // must switch representation rather than silently altering the command.
    val y = CI.workflow("edge").step("tricky", "echo 'a'  \necho b").yaml
    assert(y.contains("command: \"echo 'a'  \\necho b\""), y)
  }

  test("file names get the yml suffix") {
    assertEquals(CI.workflow("build").fileName, "build.yml")
    assertEquals(CI.workflow("build.yaml").fileName, "build.yaml")
  }
