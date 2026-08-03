import type { Machine, RunSpec } from "./types";

/** Best-effort guest OS for a machine record (kind is a boot mode for BSD). */
export function guestKind(m: Machine): "linux" | "freebsd" | "netbsd" {
  const ref = (m.image || "").toLowerCase();
  if (m.kind === "linux") return "linux";
  if (ref.startsWith("netbsd") || m.kind === "kernel") return "netbsd";
  if (ref.startsWith("freebsd") || m.kind === "firmware") return "freebsd";
  return "linux";
}

/**
 * Rebuild a RunSpec from a stopped machine's recorded fields so it can be
 * launched again (detached). bsdkrun has no `start <id>`; re-running with the
 * same image/kind/resources/volume is the Docker-`run`-style equivalent.
 *
 * We deliberately DON'T replay the recorded command: a detached machine with a
 * one-shot command powers off as soon as that command exits ("quits quickly").
 * Starting with no command keeps a console shell alive, so the machine stays
 * running until it's explicitly stopped — which is what the Start button means.
 */
export function specFromMachine(m: Machine): RunSpec {
  const kind = guestKind(m);
  // BSD images are cached as `freebsd-15.1` / `netbsd-10.1`.
  const ver = (m.image || "").match(/(\d[\w.]*)$/);
  return {
    kind,
    image: kind === "linux" ? m.image || null : null,
    version: kind !== "linux" && ver ? ver[1] : null,
    cpus: m.cpus ?? null,
    mem: m.mem ?? null,
    volume: m.volume ?? null,
    no_net: false,
    initramfs: false,
    entrypoint: null,
    mounts: [],
    ports: [],
    command: [], // persistent boot — never a one-shot command
  };
}
