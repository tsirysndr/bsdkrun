package bsdkrun

import (
	"fmt"
	"sort"
	"strconv"
)

// NetOpts is the network half of a CreateSpec: disable networking, forward
// ports, pin the MAC, or join a global network.
type NetOpts struct {
	// Disabled turns guest networking off entirely (--no-net).
	Disabled bool
	// Ports are host->guest TCP forwards, each "HOST:GUEST".
	Ports []string
	Mac   string
	// Network is a global network to join (shared subnet, name resolution).
	Network string
}

// CreateSpec carries every option BuildCreateArgs understands, keyed on OS.
// Which fields apply depends on the guest kind — exactly like the Python
// SDK's keyword grab bag — and inapplicable fields are simply ignored. The
// fluent builders (Linux, FreeBSD, ...) fill this in; it is exported so
// argv building stays testable and usable on its own.
type CreateSpec struct {
	// OS selects the guest kind: linux, freebsd, netbsd, firmware, kernel,
	// unikraft, solo5, nanos, osv.
	OS string

	// Common across kinds.
	Name string
	Cpus int
	Mem  int
	Net  *NetOpts

	// Disk-backed kinds (freebsd/netbsd/firmware/kernel; nanos and osv take
	// Persist only).
	Persist    bool
	Volume     string
	AttachDisk []string

	// linux (Image also nanos/osv; Kernel also kernel/nanos).
	Image         string
	Kernel        string
	KernelVersion string
	// Initramfs is the bare --initramfs flag (linux only). For the kernel
	// and unikraft kinds — where --initramfs takes a path — use
	// InitramfsPath instead; Python overloads one keyword for both, which a
	// typed struct cannot.
	Initramfs  bool
	Mounts     []string
	Entrypoint string
	// Env is the guest environment for the entrypoint (-e K=V). It is merged
	// over the image's own config, so a key the image already defines is
	// replaced rather than duplicated.
	Env     map[string]string
	Console string
	Command []string

	// freebsd / netbsd.
	Version  string
	Firmware string // also the firmware kind's loader
	Force    bool

	// firmware / kernel / osv.
	Disk          string
	Format        string
	InitramfsPath string
	Cmdline       string // also nanos/unikraft

	// osv.
	Gic string

	// unikraft / solo5.
	Path  string
	Block []string
	// Args are the solo5 unikernel's own arguments (after a literal "--").
	Args []string
}

func netArgs(net *NetOpts) []string {
	var a []string
	if net == nil {
		return a
	}
	if net.Disabled {
		a = append(a, "--no-net")
	}
	for _, port := range net.Ports {
		a = append(a, "--port", port)
	}
	if net.Mac != "" {
		a = append(a, "--mac", net.Mac)
	}
	if net.Network != "" {
		a = append(a, "--network", net.Network)
	}
	return a
}

func nameArgs(spec *CreateSpec) []string {
	if spec.Name != "" {
		return []string{"--name", spec.Name}
	}
	return nil
}

func vmArgs(spec *CreateSpec) []string {
	var a []string
	if spec.Cpus != 0 {
		a = append(a, "--cpus", strconv.Itoa(spec.Cpus))
	}
	if spec.Mem != 0 {
		a = append(a, "--mem", strconv.Itoa(spec.Mem))
	}
	return a
}

func diskArgs(spec *CreateSpec) []string {
	var a []string
	if spec.Persist {
		a = append(a, "--persist")
	}
	if spec.Volume != "" {
		a = append(a, "-v", spec.Volume)
	}
	for _, disk := range spec.AttachDisk {
		a = append(a, "--attach-disk", disk)
	}
	return a
}

func requireField(value, field, osKind string) error {
	if value == "" {
		return fmt.Errorf("os %q requires a %q option", osKind, field)
	}
	return nil
}

// BuildCreateArgs builds the full bsdkrun argv (minus the binary and global
// flags) for a create. Every path ends with -d (detached) so create yields a
// handle. It mirrors the Python SDK's build_create_args exactly, including
// argument ordering.
// envArgs emits `-e K=V` per entry, sorted by key.
//
// A Go map has no iteration order at all, so sorting is what keeps the argv —
// and the tests that assert on it — deterministic. The guest sees the same
// environment either way.
func envArgs(env map[string]string) []string {
	keys := make([]string, 0, len(env))
	for k := range env {
		keys = append(keys, k)
	}
	sort.Strings(keys)
	a := make([]string, 0, len(keys)*2)
	for _, k := range keys {
		a = append(a, "-e", k+"="+env[k])
	}
	return a
}

func BuildCreateArgs(spec CreateSpec) ([]string, error) {
	tail := func(a []string) []string {
		a = append(a, netArgs(spec.Net)...)
		a = append(a, nameArgs(&spec)...)
		return append(a, vmArgs(&spec)...)
	}

	switch spec.OS {
	case "linux":
		if err := requireField(spec.Image, "image", spec.OS); err != nil {
			return nil, err
		}
		a := []string{"linux", spec.Image, "-d"}
		if spec.Kernel != "" {
			a = append(a, "--kernel", spec.Kernel)
		}
		if spec.KernelVersion != "" {
			a = append(a, "--kernel-version", spec.KernelVersion)
		}
		if spec.Initramfs {
			a = append(a, "--initramfs")
		}
		if spec.Volume != "" {
			a = append(a, "-v", spec.Volume)
		}
		for _, mount := range spec.Mounts {
			a = append(a, "--mount", mount)
		}
		for _, disk := range spec.AttachDisk {
			a = append(a, "--attach-disk", disk)
		}
		if spec.Entrypoint != "" {
			a = append(a, "--entrypoint", spec.Entrypoint)
		}
		a = append(a, envArgs(spec.Env)...)
		if spec.Console != "" {
			a = append(a, "--console", spec.Console)
		}
		a = tail(a)
		if len(spec.Command) > 0 {
			a = append(a, "--")
			a = append(a, spec.Command...)
		}
		return a, nil

	case "freebsd":
		a := []string{"freebsd", "-d"}
		if spec.Version != "" {
			a = append(a, "--version", spec.Version)
		}
		if spec.Firmware != "" {
			a = append(a, "--firmware", spec.Firmware)
		}
		if spec.Force {
			a = append(a, "--force")
		}
		a = append(a, diskArgs(&spec)...)
		return tail(a), nil

	case "netbsd":
		a := []string{"netbsd", "-d"}
		if spec.Version != "" {
			a = append(a, "--version", spec.Version)
		}
		if spec.Force {
			a = append(a, "--force")
		}
		a = append(a, diskArgs(&spec)...)
		return tail(a), nil

	case "firmware":
		if err := requireField(spec.Firmware, "firmware", spec.OS); err != nil {
			return nil, err
		}
		if err := requireField(spec.Disk, "disk", spec.OS); err != nil {
			return nil, err
		}
		a := []string{"firmware", "--firmware", spec.Firmware, "--disk", spec.Disk, "-d"}
		a = append(a, diskArgs(&spec)...)
		return tail(a), nil

	case "kernel":
		if err := requireField(spec.Kernel, "kernel", spec.OS); err != nil {
			return nil, err
		}
		a := []string{"kernel", "--kernel", spec.Kernel, "-d"}
		if spec.Format != "" {
			a = append(a, "--format", spec.Format)
		}
		if spec.InitramfsPath != "" {
			a = append(a, "--initramfs", spec.InitramfsPath)
		}
		if spec.Cmdline != "" {
			a = append(a, "--cmdline", spec.Cmdline)
		}
		if spec.Disk != "" {
			a = append(a, "--disk", spec.Disk)
		}
		a = append(a, diskArgs(&spec)...)
		return tail(a), nil

	case "nanos":
		if err := requireField(spec.Image, "image", spec.OS); err != nil {
			return nil, err
		}
		a := []string{"nanos", "-d"}
		if spec.Kernel != "" {
			a = append(a, "--kernel", spec.Kernel)
		}
		if spec.Cmdline != "" {
			a = append(a, "--cmdline", spec.Cmdline)
		}
		if spec.Persist {
			a = append(a, "--persist")
		}
		a = tail(a)
		return append(a, spec.Image), nil

	case "osv":
		// Like nanos: no agent, so no volume/repo/command. OSv does have a
		// root filesystem, so --persist applies.
		if err := requireField(spec.Image, "image", spec.OS); err != nil {
			return nil, err
		}
		a := []string{"osv", "-d"}
		if spec.Cmdline != "" {
			a = append(a, "--cmdline", spec.Cmdline)
		}
		if spec.Disk != "" {
			a = append(a, "--disk", spec.Disk)
		}
		if spec.Gic != "" {
			a = append(a, "--gic", spec.Gic)
		}
		if spec.Persist {
			a = append(a, "--persist")
		}
		a = tail(a)
		return append(a, spec.Image), nil

	case "unikraft":
		// No diskArgs: a unikernel has no disk, so there is nothing to
		// persist, attach or clone.
		a := []string{"unikraft", "-d"}
		if spec.Cmdline != "" {
			a = append(a, "--cmdline", spec.Cmdline)
		}
		if spec.InitramfsPath != "" {
			a = append(a, "--initramfs", spec.InitramfsPath)
		}
		// Volumes are the exception to "no disk options": virtio-fs shares,
		// which need neither a disk nor an agent.
		for _, mount := range spec.Mounts {
			a = append(a, "--mount", mount)
		}
		a = tail(a)
		path := spec.Path
		if path == "" {
			path = "."
		}
		return append(a, path), nil

	case "solo5":
		// Like unikraft, no diskArgs — and not even mounts: a Solo5
		// unikernel declares its devices in its own MFT1 manifest, so only
		// the block backing files are passed. Guest args go last, after a
		// literal "--" — MirageOS options look like bsdkrun's own
		// (e.g. --ipv4=...), so the CLI takes them as trailing args.
		a := []string{"solo5", "-d"}
		for _, block := range spec.Block {
			a = append(a, "--block", block)
		}
		a = tail(a)
		path := spec.Path
		if path == "" {
			path = "."
		}
		a = append(a, path)
		if len(spec.Args) > 0 {
			a = append(a, "--")
			a = append(a, spec.Args...)
		}
		return a, nil
	}

	return nil, fmt.Errorf(
		"unknown os %q; expected one of linux, freebsd, netbsd, firmware, kernel, unikraft, solo5, nanos, osv",
		spec.OS,
	)
}
