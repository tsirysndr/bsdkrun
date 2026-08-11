// Package kraftfile generates a Kraftfile from a plan.Plan.
//
// The kconfig block below is the "library/base verbatim" section every
// examples/unikraft-*/Kraftfile in this repo carries — confirmed
// byte-identical across languages from Node to Rust — plus the small set of
// lines above it needed for libkrun specifically (PL011 console, FPSIMD,
// virtio-rng). Copied here verbatim (from examples/unikraft-expressjs's
// Kraftfile) rather than derived, so this is the one place that block lives
// as data instead of being hand-copied into a 21st example.
package kraftfile

import (
	"bytes"
	"fmt"
	"os"
	"path/filepath"
	"sort"
	"text/template"

	"github.com/tsirysndr/bsdkrun/pack/internal/plan"
)

const tmpl = `spec: v0.6

name: {{.Name}}

unikraft:
  version: staging
  kconfig:
    # libkrun's aarch64 console is a PL011.
    CONFIG_LIBPL011: 'y'
    CONFIG_LIBPL011_EARLY_CONSOLE: 'y'

    # Let the application use FP/SIMD (arm64-only, default n — without it a
    # binary traps on its first NEON instruction about a second into boot).
    CONFIG_FPSIMD: 'y'

    # Entropy from the host over virtio-rng — needs the driver installed by
    # pack's embedded copy of library/unikraft-base/patches.
    CONFIG_LIBVIRTIO_RNG: 'y'

    # --- everything below is library/base's config, verbatim ---
    CONFIG_HAVE_PAGING_DIRECTMAP: 'y'
    CONFIG_HAVE_PAGING: 'y'
    CONFIG_I8042: 'y'
    CONFIG_LIBVIRTIO_MMIO: 'y'
    CONFIG_VIRTIO_MMIO_MAX_DEV_CMDLINE: '8'
    CONFIG_LIBDEVFS_AUTOMOUNT: 'y'
    CONFIG_LIBDEVFS_DEV_NULL: 'y'
    CONFIG_LIBDEVFS_DEV_STDOUT: 'y'
    CONFIG_LIBDEVFS_DEV_ZERO: 'y'
    CONFIG_LIBDEVFS: 'y'
    CONFIG_LIBPOSIX_ENVIRON: 'y'
    CONFIG_LIBPOSIX_ENVIRON_ENVP0: "PATH=/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin"
    CONFIG_LIBPOSIX_ENVIRON_ENVP1: "LD_LIBRARY_PATH=/usr/local/lib:/usr/lib:/lib"
    CONFIG_LIBPOSIX_ENVIRON_ENVP2: "HOME=/"
    CONFIG_LIBPOSIX_ENVIRON_ENVP3: "PWD=/"
    CONFIG_LIBPOSIX_ENVIRON_LIBPARAM: 'y'
    CONFIG_LIBPOSIX_EVENTFD: 'y'
    CONFIG_LIBPOSIX_FDIO: 'y'
    CONFIG_LIBPOSIX_FDTAB: 'y'
    CONFIG_LIBPOSIX_FUTEX: 'y'
    CONFIG_LIBPOSIX_MMAP: 'y'
    CONFIG_LIBPOSIX_NETLINK: 'y'
    CONFIG_LIBPOSIX_PIPE: 'y'
    CONFIG_LIBPOSIX_POLL: 'y'
    CONFIG_LIBPOSIX_PROCESS: 'y'
    CONFIG_LIBPOSIX_PROCESS_MULTITHREADING: 'y'
    CONFIG_LIBPOSIX_SOCKET: 'y'
    CONFIG_LIBPOSIX_SYSINFO: 'y'
    CONFIG_LIBPOSIX_TIME: 'y'
    CONFIG_LIBPOSIX_TIMERFD: 'y'
    CONFIG_LIBPOSIX_UNIXSOCKET: 'y'
    CONFIG_LIBPOSIX_USER_GID: 0
    CONFIG_LIBPOSIX_USER_GROUPNAME: "root"
    CONFIG_LIBPOSIX_USER_UID: 0
    CONFIG_LIBPOSIX_USER_USERNAME: "root"
    CONFIG_LIBPOSIX_USER: 'y'
    CONFIG_LIBRAMFS: 'y'
    CONFIG_LIBSYSCALL_SHIM_HANDLER_ULTLS: 'y'
    CONFIG_LIBSYSCALL_SHIM_HANDLER: 'y'
    CONFIG_LIBSYSCALL_SHIM_LEGACY_VERBOSE: 'y'
    CONFIG_LIBSYSCALL_SHIM_STRACE: '{{if .Strace}}y{{else}}n{{end}}'
    CONFIG_LIBSYSCALL_SHIM: 'y'
    CONFIG_LIBUKALLOCPOOL: 'y'
    CONFIG_LIBUKBLKDEV_SYNC_IO_BLOCKED_WAITING: 'y'
    CONFIG_LIBUKBLKDEV: 'y'
    CONFIG_LIBUKBOOT_BANNER_MINIMAL: 'y'
    CONFIG_LIBUKBOOT_HEAP_BASE: '0x400000000'
    CONFIG_LIBUKBOOT_MAINTHREAD: 'y'
    CONFIG_LIBUKBOOT_SHUTDOWNREQ_HANDLER: 'y'
    CONFIG_LIBUKLIBPARAM: 'y'
    CONFIG_LIBUKCPIO: 'y'
    CONFIG_LIBUKDEBUG_CRASH_SCREEN: 'y'
    CONFIG_LIBUKDEBUG_ENABLE_ASSERT: 'y'
    CONFIG_LIBUKDEBUG_PRINT_SRCNAME: 'n'
    CONFIG_LIBUKDEBUG_PRINT_TIME: 'y'
    CONFIG_LIBUKDEBUG_PRINTK_ERR: 'y'
    CONFIG_LIBUKDEBUG_PRINTK: 'y'
    CONFIG_LIBUKDEBUG: 'y'
    CONFIG_LIBUKFALLOC: 'y'
    CONFIG_LIBUKMPI: 'n'
    CONFIG_LIBUKSIGNAL: 'y'
    CONFIG_LIBUKRANDOM_DEVFS: 'y'
    CONFIG_LIBUKRANDOM: 'y'
    CONFIG_LIBUKRANDOM_GETRANDOM: 'y'
    CONFIG_LIBUKRANDOM_CMDLINE_SEED: 'y'
    CONFIG_LIBUKRANDOM_LCPU: 'y'
    CONFIG_LIBUKVMEM_DEFAULT_BASE: '0x0000001000000000'
    CONFIG_LIBUKVMEM_DEMAND_PAGE_IN_SIZE: 12
    CONFIG_LIBUKVMEM_PAGEFAULT_HANDLER_PRIO: 4
    CONFIG_LIBUKVMEM: 'y'
    CONFIG_LIBVFSCORE_AUTOMOUNT_CI_EINITRD: 'y'
    CONFIG_LIBVFSCORE_AUTOMOUNT_CI: 'y'
    CONFIG_LIBVFSCORE_AUTOMOUNT_FB: 'y'
    CONFIG_LIBVFSCORE_AUTOMOUNT_FB0_DEV: "embedded"
    CONFIG_LIBVFSCORE_AUTOMOUNT_FB0_DRIVER: "extract"
    CONFIG_LIBVFSCORE_AUTOMOUNT_FB0_MP: "/"
    CONFIG_LIBVFSCORE_AUTOMOUNT_UP: 'y'
    CONFIG_LIBVFSCORE_AUTOMOUNT: 'y'
    CONFIG_LIBVFSCORE_NONLARGEFILE: 'y'
    CONFIG_LIBVFSCORE: 'y'
    CONFIG_LIBUK9P: 'y'
    CONFIG_OPTIMIZE_DEADELIM: 'y'
    CONFIG_OPTIMIZE_LTO: 'y'
    CONFIG_PAGING: 'y'
    CONFIG_STACK_SIZE_PAGE_ORDER: 4
    CONFIG_UKPLAT_KSP_SIZE: 32768
    CONFIG_UKPLAT_MEMREGION_MAX_COUNT: 64
    CONFIG_LIBUKNETDEV: 'y'
    CONFIG_LIBUKNETDEV_EINFO_LIBPARAM: 'y'
{{- range .KconfigKeys}}
    {{.}}: {{index $.KconfigExtra .}}
{{- end}}

libraries:
  # A Rust rewrite of upstream app-elfloader. It keeps upstream's Kconfig
  # surface exactly, which is why every symbol below is unchanged from the
  # stock elfloader configuration.
  app-elfloader:
    source: https://github.com/tsirysndr/app-elfloader.git
    version: main
    kconfig:
      CONFIG_LIBPOSIX_PROCESS_ARCH_PRCTL: 'y'
      CONFIG_APPELFLOADER_CUSTOMAPPNAME: 'y'
      CONFIG_APPELFLOADER_DEBUG: '{{if .LoaderDebug}}y{{else}}n{{end}}'
      CONFIG_APPELFLOADER_STACK_NBPAGES: 128
      CONFIG_APPELFLOADER_VFSEXEC: 'y'
      CONFIG_APPELFLOADER_VFSEXEC_EXECBIT: 'y'
      CONFIG_APPELFLOADER_VFSEXEC_ENVPWD: 'y'
      CONFIG_APPELFLOADER_VFSEXEC_ENVPATH: 'y'
      CONFIG_APPELFLOADER_AUTOGEN: 'y'
      CONFIG_APPELFLOADER_AUTOGEN_ETCRESOLVCONF: 'y'
      CONFIG_APPELFLOADER_AUTOGEN_ETCHOSTS: 'y'
      CONFIG_APPELFLOADER_AUTOGEN_ETCHOSTS_LOCALHOST4: 'y'
      CONFIG_APPELFLOADER_AUTOGEN_ETCHOSTNAME: 'y'
      CONFIG_APPELFLOADER_AUTOGEN_REPLACEEXIST: 'y'
  lwip:
    source: https://github.com/unikraft/lib-lwip.git
    version: staging
    kconfig:
      CONFIG_LWIP_LOOPIF: 'y'
      CONFIG_LWIP_UKNETDEV: 'y'
      CONFIG_LWIP_LOOPBACK: 'y'
      CONFIG_LWIP_TCP: 'y'
      CONFIG_LWIP_UDP: 'y'
      CONFIG_LWIP_RAW: 'y'
      CONFIG_LWIP_WND_SCALE: 'y'
      CONFIG_LWIP_TCP_KEEPALIVE: 'y'
      CONFIG_LWIP_THREADS: 'y'
      CONFIG_LWIP_HEAP: 'y'
      CONFIG_LWIP_SOCKET: 'y'
      CONFIG_LWIP_AUTOIFACE: 'y'
      CONFIG_LWIP_IPV4: 'y'
      CONFIG_LWIP_DHCP: 'y'
      CONFIG_LWIP_WAITIFACE: 'y'
      CONFIG_LWIP_DNS: 'n'
      CONFIG_LWIP_NUM_TCPCON: 64
      CONFIG_LWIP_NUM_TCPLISTENERS: 64
      CONFIG_LWIP_ICMP: 'y'

targets:
- fc/arm64
- fc/x86_64

cmd: [{{range $i, $c := .Cmd}}{{if $i}}, {{end}}{{printf "%q" $c}}{{end}}]
`

var parsed = template.Must(template.New("Kraftfile").Parse(tmpl))

type data struct {
	Name         string
	Cmd          []string
	KconfigExtra map[string]string
	KconfigKeys  []string // sorted, so output is deterministic
	Strace       bool
	LoaderDebug  bool
}

// Options are Kraftfile-generation choices that come from the `pack`
// invocation itself rather than from the detected plan. Both default to
// `false`: each is invaluable for debugging a guest that boots but doesn't
// behave, and each is very noisy and slows the boot down (the same tradeoff
// the hand-written examples' Kraftfiles already document for these symbols).
type Options struct {
	Strace bool

	// LoaderDebug turns on app-elfloader's placement trace
	// (CONFIG_APPELFLOADER_DEBUG). Unlike Strace — which only shows
	// syscalls the application makes, so it says nothing at all about a
	// guest that dies before its first one — this prints while the loader
	// is still mapping the binary, which is what distinguishes "never
	// loaded" from "loaded, then died at the entry point".
	LoaderDebug bool
}

// Generate renders p's Kraftfile.
func Generate(p *plan.Plan, opts Options) (string, error) {
	keys := make([]string, 0, len(p.KconfigExtra))
	for k := range p.KconfigExtra {
		keys = append(keys, k)
	}
	sort.Strings(keys)

	var buf bytes.Buffer
	err := parsed.Execute(&buf, data{
		Name:         p.Name,
		Cmd:          p.Cmd,
		KconfigExtra: p.KconfigExtra,
		KconfigKeys:  keys,
		Strace:       opts.Strace,
		LoaderDebug:  opts.LoaderDebug,
	})
	if err != nil {
		return "", fmt.Errorf("rendering Kraftfile: %w", err)
	}
	return buf.String(), nil
}

// Write renders p's Kraftfile and writes it to dir/Kraftfile.
//
// Deliberately no `rootfs:` field: every caller of internal/kraft passes
// `kraft build --rootfs <dir>` explicitly (mirroring build.sh), so the field
// would never be consulted — and unlike the hand-written examples, which
// point it at a Dockerfile for a human reader, pack has no Dockerfile to
// point at.
func Write(dir string, p *plan.Plan, opts Options) error {
	content, err := Generate(p, opts)
	if err != nil {
		return err
	}
	return os.WriteFile(filepath.Join(dir, "Kraftfile"), []byte(content), 0o644)
}
