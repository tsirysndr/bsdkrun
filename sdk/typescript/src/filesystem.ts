import { stat } from "node:fs/promises";
import { BsdkrunError } from "./errors.js";
import { runCli, runCliBinary, type RunOptions } from "./process.js";

/** Options shared by every filesystem call. */
export interface FsOptions {
  /** Abort the transfer early. */
  signal?: AbortSignal;
}

/** Options for {@link FileSystem.download}. */
export interface DownloadOptions extends FsOptions {
  /**
   * Treat the guest path as a directory and copy it recursively.
   *
   * Unlike {@link FileSystem.upload}, this cannot be detected for you: the path
   * lives in the guest, and answering it would cost an extra round trip on
   * every call.
   */
  recursive?: boolean;
}

/** A filesystem operation the guest refused. */
export class FileTransferError extends BsdkrunError {
  constructor(
    message: string,
    readonly path: string,
  ) {
    super(message);
    this.name = "FileTransferError";
  }
}

/**
 * Files in a running sandbox.
 *
 * Reached as `sandbox.fs`. Every call goes through the guest's exec agent, so
 * the sandbox has to be running — there is no offline write.
 *
 * ```ts
 * await box.fs.writeFile("/app/main.py", "print('hi')");
 * const out = await box.fs.readTextFile("/app/out.json");
 * await box.fs.upload("./src", "/app/src");
 * await box.fs.download("/app/dist", "./dist", { recursive: true });
 * ```
 */
export class FileSystem {
  constructor(private readonly id: string) {}

  /**
   * Write `data` to `path` in the guest, creating parent directories as needed.
   *
   * ```ts
   * await box.fs.writeFile("/app/main.py", "print('hi')");
   * await box.fs.writeFile("/app/logo.png", pngBytes);
   * ```
   */
  async writeFile(
    path: string,
    data: string | Uint8Array,
    opts: FsOptions = {},
  ): Promise<void> {
    await this.#run(["cp", "-", `${this.id}:${path}`], path, {
      stdin: data,
      signal: opts.signal,
    });
  }

  /**
   * Read `path` from the guest as bytes.
   *
   * ```ts
   * const bytes = await box.fs.readFile("/app/logo.png");
   * ```
   */
  async readFile(path: string, opts: FsOptions = {}): Promise<Buffer> {
    const res = await runCliBinary(["cp", `${this.id}:${path}`, "-"], {
      signal: opts.signal,
    });
    if (res.exitCode !== 0) {
      throw new FileTransferError(message(res.stderr, path), path);
    }
    return res.stdout;
  }

  /** Read `path` from the guest and decode it as UTF-8. */
  async readTextFile(path: string, opts: FsOptions = {}): Promise<string> {
    return (await this.readFile(path, opts)).toString("utf8");
  }

  /**
   * Copy a host file or directory into the guest. A directory's *contents* land
   * in `remotePath`, so `upload("./src", "/app/src")` leaves the guest's
   * `/app/src` holding what `./src` holds.
   *
   * Whether it recurses is decided by looking at the local path, so callers do
   * not have to say which kind of thing they are copying.
   */
  async upload(
    localPath: string,
    remotePath: string,
    opts: FsOptions = {},
  ): Promise<void> {
    let isDir = false;
    try {
      isDir = (await stat(localPath)).isDirectory();
    } catch (e) {
      throw new FileTransferError(
        `cannot upload ${localPath}: ${(e as Error).message}`,
        localPath,
      );
    }
    const args = ["cp", ...(isDir ? ["-r"] : []), localPath, `${this.id}:${remotePath}`];
    await this.#run(args, localPath, { signal: opts.signal });
  }

  /**
   * Copy a file or directory out of the guest onto the host. Pass
   * `{ recursive: true }` for a directory.
   */
  async download(
    remotePath: string,
    localPath: string,
    opts: DownloadOptions = {},
  ): Promise<void> {
    const args = [
      "cp",
      ...(opts.recursive ? ["-r"] : []),
      `${this.id}:${remotePath}`,
      localPath,
    ];
    await this.#run(args, remotePath, { signal: opts.signal });
  }

  async #run(args: string[], path: string, opts: RunOptions): Promise<void> {
    const res = await runCli(args, opts);
    if (res.exitCode !== 0) {
      throw new FileTransferError(message(res.stderr, path), path);
    }
  }
}

/** The CLI already explains these well; strip its `Error: ` prefix. */
function message(stderr: string, path: string): string {
  const text = stderr.trim().replace(/^Error:\s*/, "");
  return text || `file transfer failed for ${path}`;
}
