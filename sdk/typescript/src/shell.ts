import { CommandFailedError } from "./errors.ts";

/** Wrapper marking a value that must be interpolated verbatim (not quoted). */
class RawValue {
  constructor(readonly value: string) {}
}

/**
 * Interpolate a value into an `sh` template without shell-quoting it. Use with
 * care — the string is spliced in as-is, so only pass trusted content.
 *
 * ```ts
 * await box.sh`ls ${raw("-la /etc")}`;
 * ```
 */
export function raw(value: string): RawValue {
  return new RawValue(value);
}

/** POSIX single-quote a single interpolated value. */
function quoteOne(value: unknown): string {
  if (value instanceof RawValue) return value.value;
  if (value === null || value === undefined) return "''";
  if (typeof value === "number" || typeof value === "boolean") {
    return String(value);
  }
  if (Array.isArray(value)) return value.map(quoteOne).join(" ");
  const s = String(value);
  // Wrap in single quotes, escaping embedded single quotes as '\''.
  return `'${s.replace(/'/g, `'\\''`)}'`;
}

/** Assemble a shell script from a tagged-template call, quoting interpolations. */
export function buildScript(
  strings: TemplateStringsArray,
  values: unknown[],
): string {
  let out = "";
  strings.forEach((chunk, i) => {
    out += chunk;
    if (i < values.length) out += quoteOne(values[i]);
  });
  return out.trim();
}

/** The captured result of running a command. */
export class CommandResult {
  constructor(
    readonly stdout: string,
    readonly stderr: string,
    readonly exitCode: number,
    readonly command: string,
  ) {}

  /** stdout with trailing newlines trimmed — the common case. */
  text(): string {
    return this.stdout.replace(/\n+$/, "");
  }

  /** Parse stdout as JSON. */
  json<T = unknown>(): T {
    return JSON.parse(this.stdout) as T;
  }

  /** Non-empty stdout lines. */
  lines(): string[] {
    return this.stdout.split("\n").filter((l) => l.length > 0);
  }

  /** Whether the command succeeded (exit 0). */
  get ok(): boolean {
    return this.exitCode === 0;
  }

  /** Throw a {@link CommandFailedError} if the command exited non-zero. */
  throwIfFailed(): this {
    if (this.exitCode !== 0) throw new CommandFailedError(this);
    return this;
  }
}

export interface ShellRunOptions {
  env?: Record<string, string>;
  signal?: AbortSignal;
}

/** Executes an assembled shell script and returns its captured result. */
export type ShellRunner = (
  script: string,
  opts: ShellRunOptions,
) => Promise<CommandResult>;

/**
 * A lazy, chainable, awaitable shell command. Runs when awaited; throws
 * {@link CommandFailedError} on a non-zero exit unless {@link nothrow} was
 * called. Mirrors the ergonomics of Bun's `$` and Deno/dax.
 */
export class ShellPromise implements PromiseLike<CommandResult> {
  #runner: ShellRunner;
  #script: string;
  #opts: ShellRunOptions = {};
  #throwOnError = true;

  constructor(runner: ShellRunner, script: string) {
    this.#runner = runner;
    this.#script = script;
  }

  /** Do not throw on a non-zero exit — inspect `.exitCode` instead. */
  nothrow(): this {
    this.#throwOnError = false;
    return this;
  }

  /** Set environment variables for this command. */
  env(vars: Record<string, string>): this {
    this.#opts.env = { ...this.#opts.env, ...vars };
    return this;
  }

  /** Abort the command via an AbortSignal. */
  signal(signal: AbortSignal): this {
    this.#opts.signal = signal;
    return this;
  }

  async #run(): Promise<CommandResult> {
    const res = await this.#runner(this.#script, this.#opts);
    if (this.#throwOnError && res.exitCode !== 0) {
      throw new CommandFailedError(res);
    }
    return res;
  }

  then<TResult1 = CommandResult, TResult2 = never>(
    onfulfilled?:
      | ((value: CommandResult) => TResult1 | PromiseLike<TResult1>)
      | null,
    onrejected?: ((reason: unknown) => TResult2 | PromiseLike<TResult2>) | null,
  ): PromiseLike<TResult1 | TResult2> {
    return this.#run().then(onfulfilled, onrejected);
  }

  /** Await and return trimmed stdout. */
  async text(): Promise<string> {
    return (await this.#run()).text();
  }

  /** Await and parse stdout as JSON. */
  async json<T = unknown>(): Promise<T> {
    return (await this.#run()).json<T>();
  }

  /** Await and return non-empty stdout lines. */
  async lines(): Promise<string[]> {
    return (await this.#run()).lines();
  }
}

/** The `sh` tagged-template function bound to a specific runner. */
export type Sh = (
  strings: TemplateStringsArray,
  ...values: unknown[]
) => ShellPromise;

/** Build an `sh` tagged-template bound to the given runner. */
export function createSh(runner: ShellRunner): Sh {
  return (strings, ...values) =>
    new ShellPromise(runner, buildScript(strings, values));
}
