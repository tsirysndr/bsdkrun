import { useMemo } from "react";
import {
  Button,
  Table,
  TableBody,
  TableCell,
  TableColumn,
  TableHeader,
  TableRow,
  Tooltip,
} from "@heroui/react";
import { useAtomValue, useSetAtom } from "jotai";
import { IconStack2, IconPlayerPlayFilled } from "@tabler/icons-react";
import { filterAtom, runOpenAtom, runPrefillAtom } from "../state/atoms";
import { useImages } from "../lib/queries";
import { ago, fullDate, humanSize, shortId } from "../lib/format";
import { EmptyState, ViewShell } from "./ViewShell";
import { TableSkeleton } from "./Skeletons";
import { useInfiniteRows } from "../hooks/useInfiniteRows";
import { useListNavigation } from "../hooks/useListNavigation";
import type { Image } from "../lib/types";

/** Guess a run kind from a fetched-image reference (freebsd-15.1 / netbsd-…). */
function refKind(ref: string): "linux" | "freebsd" | "netbsd" {
  if (ref.startsWith("freebsd")) return "freebsd";
  if (ref.startsWith("netbsd")) return "netbsd";
  return "linux";
}

export default function ImagesView() {
  const { data: images = [], isLoading } = useImages();
  const filter = useAtomValue(filterAtom).toLowerCase();
  const setRunOpen = useSetAtom(runOpenAtom);
  const setPrefill = useSetAtom(runPrefillAtom);

  const rows = useMemo(
    () =>
      images.filter(
        (im) => !filter || im.reference.toLowerCase().includes(filter),
      ),
    [images, filter],
  );
  const { visible, sentinelRef, hasMore } = useInfiniteRows(rows.length);
  const visibleRows = useMemo(() => rows.slice(0, visible), [rows, visible]);

  const runImage = (im: Image) => {
    const kind = refKind(im.reference);
    setPrefill({ kind, image: kind === "linux" ? im.reference : undefined });
    setRunOpen(true);
  };

  // ↑/↓ highlight an image; Enter or R opens the Run dialog prefilled from it.
  const { focusedId } = useListNavigation(visibleRows, (im) => im.id, {
    onEnter: runImage,
    keys: { r: runImage },
  });

  if (isLoading && images.length === 0) {
    return (
      <ViewShell title="Images" subtitle="Pulled OCI images and fetched BSD images">
        <TableSkeleton />
      </ViewShell>
    );
  }

  if (images.length === 0) {
    return (
      <ViewShell title="Images" subtitle="Pulled OCI images and fetched BSD images">
        <EmptyState
          icon={<IconStack2 size={28} />}
          title="No images yet"
          hint="Run a Linux OCI image or fetch a BSD release — it'll be cached and listed here."
          action={
            <Button color="primary" variant="shadow" onPress={() => setRunOpen(true)}>
              Run a machine
            </Button>
          }
        />
      </ViewShell>
    );
  }

  return (
    <ViewShell
      title="Images"
      subtitle={`${images.length} cached`}
      searchPlaceholder="Filter images…"
    >
      <Table
        removeWrapper
        aria-label="Images"
        classNames={{
          th: "bg-transparent text-foreground-500 border-b border-white/10 text-[11px] uppercase tracking-wider",
          td: "py-3 first:rounded-l-lg last:rounded-r-lg",
          tr: "group/tr transition-colors hover:bg-white/[0.03]",
        }}
      >
        <TableHeader>
          <TableColumn>Reference</TableColumn>
          <TableColumn>Image ID</TableColumn>
          <TableColumn>Size</TableColumn>
          <TableColumn width={150}>Created</TableColumn>
          <TableColumn align="end"> </TableColumn>
        </TableHeader>
        <TableBody>
          {visibleRows.map((im) => (
            <TableRow
              key={im.id}
              data-list-row={im.id}
              className={
                im.id === focusedId
                  ? "bg-primary/10 shadow-[inset_2px_0_0] shadow-primary"
                  : undefined
              }
            >
              <TableCell>
                <span className="text-sm font-medium text-foreground">
                  {im.reference}
                </span>
              </TableCell>
              <TableCell>
                <span className="font-mono text-[11px] text-foreground-500">
                  {shortId(im.id)}
                </span>
              </TableCell>
              <TableCell>
                <span className="text-xs text-foreground-400">
                  {humanSize(im.size)}
                </span>
              </TableCell>
              <TableCell>
                <Tooltip content={fullDate(im.created_at)} placement="top">
                  <span className="cursor-default whitespace-nowrap text-xs text-foreground-500">
                    {ago(im.created_at)}
                  </span>
                </Tooltip>
              </TableCell>
              <TableCell>
                <div className="flex justify-end">
                  <Tooltip content="Run this image" placement="top">
                    <Button
                      isIconOnly
                      size="sm"
                      variant="light"
                      color="primary"
                      onPress={() => runImage(im)}
                    >
                      <IconPlayerPlayFilled size={15} />
                    </Button>
                  </Tooltip>
                </div>
              </TableCell>
            </TableRow>
          ))}
        </TableBody>
      </Table>

      {hasMore && (
        <div
          ref={sentinelRef}
          className="flex items-center justify-center py-4 text-xs text-foreground-500"
        >
          Loading more… ({visible} of {rows.length})
        </div>
      )}
    </ViewShell>
  );
}
