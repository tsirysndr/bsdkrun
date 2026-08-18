import { mkdirSync, mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { spawnCli } from "./process.js";

/**
 * CI workflows defined in code instead of YAML.
 *
 * The builder produces exactly the file `bsdkrun ci` (and tangled's spindle)
 * consumes — {@link CIWorkflow.yaml} is that file, {@link CIWorkflow.save}
 * commits it to `.tangled/workflows/`, and {@link CIWorkflow.run} executes it
 * in a microVM without a file ever touching the repository:
 *
 * ```ts
 * await workflow("test")
 *   .onPush("main")
 *   .deps("nodejs", "pnpm")
 *   .env("CI_FROM", "sdk")
 *   .step("install", "pnpm install --frozen-lockfile")
 *   .step("test", "pnpm test")
 *   .run();
 * ```
 *
 * Code is the source of truth and YAML the wire format, in that order — which
 * is why save() writes a generated-file header: a hand-edit there will be
 * overwritten by the next save().
 */

interface Constraint {
  events: string[];
  branches: string[];
}

interface Step {
  name: string;
  command: string;
  env?: Record<string, string>;
}

interface CloneOpts {
  depth?: number;
  skip?: boolean;
}

/** Start a CI workflow definition. */
export function workflow(name: string): CIWorkflow {
  return new CIWorkflow(name);
}

export class CIWorkflow {
  private engineName = "nixery";
  private when: Constraint[] = [];
  private dependencies = new Map<string, string[]>();
  private environment = new Map<string, string>();
  private steps: Step[] = [];
  private cloneOpts?: CloneOpts;

  constructor(private readonly name: string) {}

  /** Override the engine (`nixery` by default). */
  engine(engine: string): this {
    this.engineName = engine;
    return this;
  }

  /** Add a push trigger for the given branches. */
  onPush(...branches: string[]): this {
    this.when.push({ events: ["push"], branches });
    return this;
  }

  /** Add a pull_request trigger targeting the given branches. */
  onPullRequest(...branches: string[]): this {
    this.when.push({ events: ["pull_request"], branches });
    return this;
  }

  /** Add a trigger with explicit events. */
  on(events: string[], ...branches: string[]): this {
    this.when.push({ events, branches });
    return this;
  }

  /** Add nixpkgs dependencies — the toolchain the steps run against. */
  deps(...packages: string[]): this {
    const list = this.dependencies.get("nixpkgs") ?? [];
    this.dependencies.set("nixpkgs", [...list, ...packages]);
    return this;
  }

  /** Add dependencies from a custom registry (a flake reference). */
  depsFrom(registry: string, ...packages: string[]): this {
    const list = this.dependencies.get(registry) ?? [];
    this.dependencies.set(registry, [...list, ...packages]);
    return this;
  }

  /** Set a workflow-level environment variable. */
  env(key: string, value: string): this {
    this.environment.set(key, value);
    return this;
  }

  /** Append a step; steps run serially in one VM, from the workspace root. */
  step(name: string, command: string, env?: Record<string, string>): this {
    this.steps.push({ name, command, env });
    return this;
  }

  /** Set the clone depth (default 1). */
  cloneDepth(depth: number): this {
    this.cloneOpts = { ...this.cloneOpts, depth };
    return this;
  }

  /** Skip the checkout entirely. */
  skipClone(): this {
    this.cloneOpts = { ...this.cloneOpts, skip: true };
    return this;
  }

  /** The workflow file name save() writes: `<name>.yml`. */
  fileName(): string {
    return /\.ya?ml$/.test(this.name) ? this.name : `${this.name}.yml`;
  }

  /**
   * Render the workflow file. Scalars are emitted as JSON strings — valid
   * YAML by construction — and commands as literal blocks when safe, so the
   * SDK needs no YAML dependency.
   */
  yaml(): string {
    const out: string[] = [];
    const q = (s: string) => JSON.stringify(s);

    if (this.when.length > 0) {
      out.push("when:");
      for (const c of this.when) {
        out.push(`  - event: [${c.events.map(q).join(", ")}]`);
        if (c.branches.length === 1) out.push(`    branch: ${q(c.branches[0]!)}`);
        else if (c.branches.length > 1)
          out.push(`    branch: [${c.branches.map(q).join(", ")}]`);
      }
      out.push("");
    }

    out.push(`engine: ${this.engineName}`);

    if (this.dependencies.size > 0) {
      out.push("", "dependencies:");
      for (const reg of [...this.dependencies.keys()].sort()) {
        out.push(`  ${q(reg)}:`);
        for (const p of this.dependencies.get(reg)!) out.push(`    - ${q(p)}`);
      }
    }

    if (this.environment.size > 0) {
      out.push("", "environment:");
      for (const k of [...this.environment.keys()].sort()) {
        out.push(`  ${k}: ${q(this.environment.get(k)!)}`);
      }
    }

    if (this.cloneOpts) {
      out.push("", "clone:");
      if (this.cloneOpts.skip) out.push("  skip: true");
      if (this.cloneOpts.depth) out.push(`  depth: ${this.cloneOpts.depth}`);
    }

    out.push("", "steps:");
    for (const s of this.steps) {
      out.push(`  - name: ${q(s.name)}`);
      // Literal blocks read well in a committed file, but cannot represent
      // trailing spaces or carriage returns byte-for-byte; fall back to a
      // JSON string rather than silently altering the command.
      const blockSafe =
        s.command !== "" &&
        !s.command.includes("\r") &&
        s.command.split("\n").every((l) => l === l.replace(/[ ]+$/, ""));
      if (blockSafe) {
        out.push("    command: |");
        for (const line of s.command.replace(/\n+$/, "").split("\n")) {
          out.push(`      ${line}`);
        }
      } else {
        out.push(`    command: ${q(s.command)}`);
      }
      if (s.env && Object.keys(s.env).length > 0) {
        out.push("    environment:");
        for (const k of Object.keys(s.env).sort()) {
          out.push(`      ${k}: ${q(s.env[k]!)}`);
        }
      }
    }
    return out.join("\n") + "\n";
  }

  /**
   * Write the workflow into `<repo>/.tangled/workflows/`, where spindle and
   * `bsdkrun ci` both discover it. Returns the path.
   */
  save(repo: string): string {
    const dir = join(repo, ".tangled", "workflows");
    mkdirSync(dir, { recursive: true });
    const path = join(dir, this.fileName());
    writeFileSync(
      path,
      "# Generated by the bsdkrun SDK — edit the code that save()d it instead.\n" +
        this.yaml(),
    );
    return path;
  }

  /**
   * Execute the workflow in a microVM against `dir` (the current directory
   * by default), streaming output. The YAML never touches the repository —
   * it goes to a temp file and `bsdkrun ci run -f`.
   */
  run(dir?: string): Promise<void> {
    const tmp = mkdtempSync(join(tmpdir(), "bsdkrun-ci-"));
    const file = join(tmp, this.fileName());
    writeFileSync(file, this.yaml());

    const args = ["ci", "run", "-f", file];
    if (dir) args.push("-w", dir);

    return new Promise((resolve, reject) => {
      const child = spawnCli(args);
      child.on("error", (err) => {
        rmSync(tmp, { recursive: true, force: true });
        reject(err);
      });
      child.on("exit", (code) => {
        rmSync(tmp, { recursive: true, force: true });
        if (code === 0) resolve();
        else reject(new Error(`workflow ${this.name} failed (exit ${code})`));
      });
    });
  }
}
