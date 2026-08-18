import { describe, expect, it } from "bun:test";
import { workflow } from "../src/ci.js";

// The YAML this builder emits is consumed by tangled's own workflow parser
// (inside `bsdkrun ci`), so these tests pin the emitted shape — a change here
// is a change to what spindle would receive.
describe("ci workflow builder", () => {
  it("renders the full workflow shape", () => {
    const y = workflow("test")
      .onPush("main")
      .onPullRequest("main", "develop")
      .deps("nodejs", "pnpm")
      .depsFrom("github:nix-community/fenix/abc123", "stable.defaultToolchain")
      .env("CI_FROM", "sdk")
      .cloneDepth(100)
      .step("install", "pnpm install")
      .step("test", "pnpm test", { NODE_ENV: "test" })
      .yaml();

    expect(y).toContain('  - event: ["push"]\n    branch: "main"');
    expect(y).toContain('branch: ["main", "develop"]');
    expect(y).toContain("engine: nixery");
    expect(y).toContain('"nixpkgs":\n    - "nodejs"\n    - "pnpm"');
    expect(y).toContain('"github:nix-community/fenix/abc123":');
    expect(y).toContain('CI_FROM: "sdk"');
    expect(y).toContain("depth: 100");
    expect(y).toContain('- name: "install"\n    command: |\n      pnpm install');
    expect(y).toContain('environment:\n      NODE_ENV: "test"');
  });

  it("falls back to a JSON string for block-unsafe commands", () => {
    // Trailing spaces do not survive a literal block scalar; the emitter
    // must switch representation rather than silently altering the command.
    const y = workflow("edge").step("tricky", "echo 'a'  \necho b").yaml();
    expect(y).toContain('command: "echo \'a\'  \\necho b"');
  });

  it("derives the file name", () => {
    expect(workflow("build").fileName()).toBe("build.yml");
    expect(workflow("build.yaml").fileName()).toBe("build.yaml");
  });
});
