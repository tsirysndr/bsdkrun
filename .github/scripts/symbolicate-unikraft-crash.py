#!/usr/bin/env python3
"""Resolve the addresses in a Unikraft crash banner to symbols.

Usage: symbolicate-unikraft-crash.py <console.out> <kernel.dbg> <rootfs-dir>

Two symbol domains, told apart by address:

  * Kernel text lives low (the fc image links around 1 MiB on x86_64,
    0x80000000 on arm64) and resolves against the .dbg via addr2line.
  * Application mappings live in ukvmem's range (0x10_0000_0000+). Their
    bases are not fixed: they are whatever the ELF loader printed in its
    "loaded to" lines (CONFIG_APPELFLOADER_DEBUG), so those lines are parsed
    from the same console output and each address is attributed to the
    mapping that contains it, then resolved against the matching file in the
    exported rootfs -- the application binary or its interpreter -- via the
    dynamic symbol table (`nm -D`), which survives stripping.

Exits 0 always: this is a diagnostic, not a gate.
"""

import re
import subprocess
import sys

APP_RANGE_START = 0x10_0000_0000


def run(cmd):
    try:
        return subprocess.run(cmd, capture_output=True, text=True).stdout
    except FileNotFoundError:
        # binutils absent (e.g. a macOS host); resolve what we can without it.
        return ""


def addr2line(dbg, addr):
    out = run(["addr2line", "-f", "-e", dbg, hex(addr)]).splitlines()
    if out and not out[0].startswith("??"):
        return " ".join(out)
    return None


def load_dynsyms(path):
    syms = []
    for line in run(["nm", "-D", "--defined-only", path]).splitlines():
        parts = line.split()
        if len(parts) < 3:
            continue
        try:
            syms.append((int(parts[0], 16), parts[2]))
        except ValueError:
            continue
    syms.sort()
    return syms


def nearest(syms, off):
    best = None
    for a, name in syms:
        if a <= off:
            best = (a, name)
        else:
            break
    return best


def main():
    console, dbg, rootfs = sys.argv[1], sys.argv[2], sys.argv[3]
    with open(console, "rb") as f:
        text = f.read().replace(b"\x00", b"").decode("utf-8", "replace")

    # The loader prints one line per mapping:
    #   app: loaded to 0x10010c0000-0x10018ae000 (...), entry at 0x...
    #   exec: loaded to 0x1001db0000-0x1001ea1000 (...), entry at 0x...
    #   <interp>: loaded to 0x10018c0000-0x1001983000, bias 0x..., entry ...
    # Later mappings shadow earlier ones at the same range, which is what we
    # want: the most recent exec is the one that was running.
    maps = []
    for m in re.finditer(
        r"(app|exec|<interp>): loaded to 0x([0-9a-f]+)-0x([0-9a-f]+)", text
    ):
        kind, lo, hi = m.group(1), int(m.group(2), 16), int(m.group(3), 16)
        if kind == "<interp>":
            import glob

            cands = glob.glob(rootfs + "/lib/ld-musl-*.so.1")
            path = cands[0] if cands else None
        else:
            path = None  # filled in below from the "loading X as Y" line
        maps.append({"kind": kind, "lo": lo, "hi": hi, "path": path})

    # Which binary each app/exec mapping is: the loader names it just before.
    names = re.findall(r"(?:loading|binfmt: loading) (\S+) as ", text)
    it = iter(names)
    for mp in maps:
        if mp["kind"] != "<interp>":
            mp["path"] = rootfs + next(it, "/usr/local/bin/postgres")

    # Addresses out of the register dump (banner plus the 20 lines after it).
    banner = text[text.find("Unikraft Crash"):]
    banner = "\n".join(banner.splitlines()[:24])
    addrs = sorted({int(a, 16) for a in re.findall(r"\b([0-9a-f]{12,16})\b", banner)})

    dynsym_cache = {}
    resolved = 0
    for addr in addrs:
        if addr < APP_RANGE_START:
            sym = addr2line(dbg, addr)
            if sym:
                print(f"0x{addr:x} -> [kernel] {sym}")
                resolved += 1
            continue
        for mp in reversed(maps):
            if mp["lo"] <= addr < mp["hi"] and mp["path"]:
                if mp["path"] not in dynsym_cache:
                    dynsym_cache[mp["path"]] = load_dynsyms(mp["path"])
                hit = nearest(dynsym_cache[mp["path"]], addr - mp["lo"])
                if hit:
                    off = addr - mp["lo"] - hit[0]
                    print(
                        f"0x{addr:x} -> [{mp['path'].rsplit('/', 1)[-1]}"
                        f" @ 0x{mp['lo']:x}] {hit[1]}+0x{off:x}"
                    )
                    resolved += 1
                break
    if not resolved:
        print("no addresses resolved (is CONFIG_APPELFLOADER_DEBUG on?)")


if __name__ == "__main__":
    main()
    sys.exit(0)
