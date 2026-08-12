// Small display helpers shared across views.

import dayjs from "dayjs";
import relativeTime from "dayjs/plugin/relativeTime";
import localizedFormat from "dayjs/plugin/localizedFormat";

dayjs.extend(relativeTime);
dayjs.extend(localizedFormat);

/**
 * bsdkrun stamps timestamps as a Unix epoch in **seconds** (stringified, e.g.
 * "1754239152"). Older rows may use SQL-style `YYYY-MM-DD HH:MM:SS`. Handle both
 * so dayjs doesn't misread an epoch as a calendar year.
 */
function parse(ts: string | null | undefined) {
  if (ts == null) return null;
  const s = String(ts).trim();
  if (!s) return null;
  if (/^\d+$/.test(s)) {
    const n = Number(s);
    const d = s.length >= 13 ? dayjs(n) : dayjs.unix(n); // 13+ digits ⇒ ms
    return d.isValid() ? d : null;
  }
  const d = dayjs(s.replace(" ", "T"));
  return d.isValid() ? d : null;
}

export function humanSize(bytes: number | null | undefined): string {
  if (bytes == null || bytes < 0) return "—";
  if (bytes === 0) return "0 B";
  const units = ["B", "KB", "MB", "GB", "TB"];
  const i = Math.floor(Math.log(bytes) / Math.log(1024));
  const v = bytes / Math.pow(1024, i);
  return `${v.toFixed(v >= 10 || i === 0 ? 0 : 1)} ${units[i]}`;
}

/** Relative "ago" string from an ISO/SQL timestamp (via dayjs). */
export function ago(ts: string | null | undefined): string {
  const d = parse(ts);
  return d ? d.fromNow() : "—";
}

export function shortId(id: string): string {
  return id.length > 12 ? id.slice(0, 12) : id;
}

/**
 * Human status for an exited machine. A clean stop lands on 143 (SIGTERM) or
 * 137 (SIGKILL) — show "Stopped", not a scary code. 0 is a normal exit; any
 * other non-zero is a real failure worth surfacing with its code.
 */
export function exitLabel(code: number | null | undefined): string {
  if (code == null) return "Exited";
  if (code === 143 || code === 137) return "Stopped";
  if (code === 0) return "Exited";
  return `Exited (${code})`;
}

/** Full, human-readable timestamp for tooltips (e.g. "August 3, 2026 6:39:12 PM"). */
export function fullDate(ts: string | null | undefined): string {
  const d = parse(ts);
  return d ? d.format("LLLL") : ts || "Unknown";
}

const BADGE = {
  linux: {
    label: "Linux",
    className: "bg-sky-500/15 text-sky-300 border-sky-500/25",
    dot: "bg-sky-400",
  },
  freebsd: {
    label: "FreeBSD",
    className: "bg-red-500/15 text-red-300 border-red-500/25",
    dot: "bg-red-400",
  },
  netbsd: {
    label: "NetBSD",
    className: "bg-orange-500/15 text-orange-300 border-orange-500/25",
    dot: "bg-orange-400",
  },
  unikraft: {
    label: "Unikraft",
    className: "bg-teal-500/15 text-teal-300 border-teal-500/25",
    dot: "bg-teal-400",
  },
  nanos: {
    label: "Nanos",
    className: "bg-amber-500/15 text-amber-300 border-amber-500/25",
    dot: "bg-amber-400",
  },
  osv: {
    label: "OSv",
    className: "bg-rose-500/15 text-rose-300 border-rose-500/25",
    dot: "bg-rose-400",
  },
  solo5: {
    label: "Solo5",
    className: "bg-lime-500/15 text-lime-300 border-lime-500/25",
    dot: "bg-lime-400",
  },
} as const;

/**
 * Whether this machine is a unikernel (Unikraft, Nanos, OSv or Solo5) — the
 * application linked into, or loaded by, the kernel. There is no in-guest
 * agent, so snapshots and the terminal/exec do not apply and the CLI rejects
 * them; the UI hides those actions rather than offering a button that always
 * errors.
 */
export function isUnikraft(kind: string | null | undefined): boolean {
  return (
    kind === "unikraft" || kind === "nanos" || kind === "osv" || kind === "solo5"
  );
}

/**
 * A themed badge per guest OS. bsdkrun records BSD machines by *boot mode*
 * (`firmware` for FreeBSD-on-macOS, `kernel` for NetBSD), so we prefer the
 * image reference (`freebsd-15.1` / `netbsd-10.1`) and fall back to the kind.
 */
export function kindColor(
  kind: string,
  image?: string | null,
): { label: string; className: string; dot: string } {
  const ref = (image || "").toLowerCase();
  if (ref.startsWith("freebsd") || kind === "freebsd" || kind === "firmware") {
    return BADGE.freebsd;
  }
  if (ref.startsWith("netbsd") || kind === "netbsd" || kind === "kernel") {
    return BADGE.netbsd;
  }
  if (kind === "unikraft") return BADGE.unikraft;
  if (kind === "nanos" || ref.startsWith("nanos")) return BADGE.nanos;
  if (kind === "osv" || ref.startsWith("osv")) return BADGE.osv;
  if (kind === "solo5") return BADGE.solo5;
  if (kind === "linux") return BADGE.linux;
  return {
    label: kind || "machine",
    className: "bg-violet-500/15 text-violet-300 border-violet-500/25",
    dot: "bg-violet-400",
  };
}
