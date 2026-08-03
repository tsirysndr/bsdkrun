import { CommandFailedError } from "./errors.ts";
import { runCli } from "./process.ts";
import { CommandResult } from "./shell.ts";
import type { ImageInfo } from "./types.ts";

function mapImage(row: Record<string, unknown>): ImageInfo {
  return {
    id: String(row.id),
    reference: String(row.reference),
    digest: String(row.digest),
    size: Number(row.size),
    rootfs: String(row.rootfs),
    createdAt: Number(row.created_at),
  };
}

/** List downloaded images (pulled OCI images + fetched BSD images). */
export async function listImages(): Promise<ImageInfo[]> {
  const res = await runCli(["images", "--json"]);
  if (res.exitCode !== 0) {
    throw new CommandFailedError(
      new CommandResult(res.stdout, res.stderr, res.exitCode, "bsdkrun images"),
    );
  }
  const rows = JSON.parse(res.stdout || "[]") as Record<string, unknown>[];
  return rows.map(mapImage);
}

/** Namespace grouping image operations. */
export const images = {
  list: listImages,
};
