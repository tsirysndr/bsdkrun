package bsdkrun

import (
	"fmt"
	"strconv"
	"strings"
)

// CreateBuilder configures a new microVM. Build one with a guest-kind
// constructor (Linux, FreeBSD, NetBSD, Firmware, Kernel, Nanos, OSv,
// Unikraft, Solo5), chain options, then Create:
//
//	box, err := bsdkrun.Linux("alpine").
//		Cpus(2).Mem(1024).
//		Volume("web").
//		Mount("~/project:/src").
//		Port("8080:80").
//		Command("sleep", "300").
//		Create()
//
// One builder type serves every guest kind — exactly like the Python SDK's
// keyword grab bag — and options that don't apply to the chosen kind are
// simply ignored by the argv builder.
type CreateBuilder struct {
	spec       CreateSpec
	logLevel   int
	hasLogUser bool
}

// Linux runs an OCI image as a Linux microVM (docker run-style).
func Linux(image string) *CreateBuilder {
	return &CreateBuilder{spec: CreateSpec{OS: "linux", Image: image}}
}

// FreeBSD runs FreeBSD (EFI on macOS, PVH on Linux/amd64).
func FreeBSD() *CreateBuilder {
	return &CreateBuilder{spec: CreateSpec{OS: "freebsd"}}
}

// NetBSD runs NetBSD (direct-kernel boot everywhere).
func NetBSD() *CreateBuilder {
	return &CreateBuilder{spec: CreateSpec{OS: "netbsd"}}
}

// Firmware boots a raw disk through its UEFI loader.
func Firmware(firmware, disk string) *CreateBuilder {
	return &CreateBuilder{spec: CreateSpec{OS: "firmware", Firmware: firmware, Disk: disk}}
}

// Kernel boots a kernel directly, no bootloader.
func Kernel(kernel string) *CreateBuilder {
	return &CreateBuilder{spec: CreateSpec{OS: "kernel", Kernel: kernel}}
}

// Nanos boots a Nanos unikernel image (a path, or a bare name in
// ~/.ops/images — what `ops build -i` makes).
func Nanos(image string) *CreateBuilder {
	return &CreateBuilder{spec: CreateSpec{OS: "nanos", Image: image}}
}

// OSv boots an OSv image (an aarch64 loader.img, or on x86_64 the loader
// ELF plus a Disk).
func OSv(image string) *CreateBuilder {
	return &CreateBuilder{spec: CreateSpec{OS: "osv", Image: image}}
}

// Unikraft boots a Unikraft unikernel: a kraft project directory or a built
// image ("" defaults to ".").
func Unikraft(path string) *CreateBuilder {
	return &CreateBuilder{spec: CreateSpec{OS: "unikraft", Path: path}}
}

// Solo5 boots a Solo5 (MirageOS) unikernel under the solo5-hvt tender: a
// .hvt binary or a project directory whose dist/ holds one ("" defaults to
// ".").
func Solo5(path string) *CreateBuilder {
	return &CreateBuilder{spec: CreateSpec{OS: "solo5", Path: path}}
}

// CreateFrom builds directly from a CreateSpec, for callers that assembled
// one by hand.
func CreateFrom(spec CreateSpec) *CreateBuilder {
	return &CreateBuilder{spec: spec}
}

// -- common options ---------------------------------------------------------

// Name names the machine.
func (b *CreateBuilder) Name(name string) *CreateBuilder {
	b.spec.Name = name
	return b
}

// Cpus sets the vCPU count.
func (b *CreateBuilder) Cpus(n int) *CreateBuilder {
	b.spec.Cpus = n
	return b
}

// Mem sets the RAM in MiB.
func (b *CreateBuilder) Mem(mib int) *CreateBuilder {
	b.spec.Mem = mib
	return b
}

func (b *CreateBuilder) net() *NetOpts {
	if b.spec.Net == nil {
		b.spec.Net = &NetOpts{}
	}
	return b.spec.Net
}

// Port forwards a host TCP port to the guest ("HOST:GUEST"). Repeatable.
func (b *CreateBuilder) Port(hostGuest string) *CreateBuilder {
	n := b.net()
	n.Ports = append(n.Ports, hostGuest)
	return b
}

// Forward is Port with numeric halves.
func (b *CreateBuilder) Forward(host, guest int) *CreateBuilder {
	return b.Port(fmt.Sprintf("%d:%d", host, guest))
}

// Mac pins the guest MAC address.
func (b *CreateBuilder) Mac(mac string) *CreateBuilder {
	b.net().Mac = mac
	return b
}

// Network joins a global network (shared subnet, name resolution).
func (b *CreateBuilder) Network(name string) *CreateBuilder {
	b.net().Network = name
	return b
}

// NoNet disables guest networking entirely.
func (b *CreateBuilder) NoNet() *CreateBuilder {
	b.net().Disabled = true
	return b
}

// -- disk options -----------------------------------------------------------

// Persist keeps the root disk across runs (freebsd/netbsd/nanos/osv).
func (b *CreateBuilder) Persist() *CreateBuilder {
	b.spec.Persist = true
	return b
}

// Volume uses a named persistent volume as the rootfs (-v).
func (b *CreateBuilder) Volume(name string) *CreateBuilder {
	b.spec.Volume = name
	return b
}

// AttachDisk attaches an extra disk ("PATH" or "PATH:ro"). Repeatable.
func (b *CreateBuilder) AttachDisk(disk string) *CreateBuilder {
	b.spec.AttachDisk = append(b.spec.AttachDisk, disk)
	return b
}

// -- linux options ----------------------------------------------------------

// Kernel overrides the guest kernel (linux; also the nanos kernel on Linux
// hosts).
func (b *CreateBuilder) Kernel(path string) *CreateBuilder {
	b.spec.Kernel = path
	return b
}

// KernelVersion picks a packaged kernel version (linux).
func (b *CreateBuilder) KernelVersion(version string) *CreateBuilder {
	b.spec.KernelVersion = version
	return b
}

// Initramfs boots via an initramfs (the bare linux flag). For the kernel
// and unikraft kinds — where --initramfs takes a path — use InitramfsPath.
func (b *CreateBuilder) Initramfs() *CreateBuilder {
	b.spec.Initramfs = true
	return b
}

// InitramfsPath sets the initramfs image path (kernel/unikraft kinds).
func (b *CreateBuilder) InitramfsPath(path string) *CreateBuilder {
	b.spec.InitramfsPath = path
	return b
}

// Mount shares a host directory ("HOST:GUEST", optionally ":ro").
// Repeatable. For unikraft these are virtio-fs shares.
func (b *CreateBuilder) Mount(hostGuest string) *CreateBuilder {
	b.spec.Mounts = append(b.spec.Mounts, hostGuest)
	return b
}

// Entrypoint overrides the image entrypoint (linux).
func (b *CreateBuilder) Entrypoint(entrypoint string) *CreateBuilder {
	b.spec.Entrypoint = entrypoint
	return b
}

// Console selects the guest console device (linux).
func (b *CreateBuilder) Console(console string) *CreateBuilder {
	b.spec.Console = console
	return b
}

// Command sets the guest command (the argv after `--`).
func (b *CreateBuilder) Command(argv ...string) *CreateBuilder {
	b.spec.Command = argv
	return b
}

// -- BSD options ------------------------------------------------------------

// Version picks the OS release to fetch/boot (freebsd/netbsd).
func (b *CreateBuilder) Version(version string) *CreateBuilder {
	b.spec.Version = version
	return b
}

// Firmware overrides the UEFI firmware (freebsd).
func (b *CreateBuilder) Firmware(path string) *CreateBuilder {
	b.spec.Firmware = path
	return b
}

// Force re-fetches/rebuilds the image even when cached.
func (b *CreateBuilder) Force() *CreateBuilder {
	b.spec.Force = true
	return b
}

// -- kernel / nanos / osv / unikraft options --------------------------------

// Format sets the kernel image format (kernel kind, e.g. "elf").
func (b *CreateBuilder) Format(format string) *CreateBuilder {
	b.spec.Format = format
	return b
}

// Cmdline sets the kernel command line (kernel/nanos/osv/unikraft).
func (b *CreateBuilder) Cmdline(cmdline string) *CreateBuilder {
	b.spec.Cmdline = cmdline
	return b
}

// Disk sets the root disk (kernel/osv).
func (b *CreateBuilder) Disk(path string) *CreateBuilder {
	b.spec.Disk = path
	return b
}

// Gic picks the interrupt controller revision, "v2" or "v3" (osv, aarch64
// only).
func (b *CreateBuilder) Gic(gic string) *CreateBuilder {
	b.spec.Gic = gic
	return b
}

// -- solo5 options ----------------------------------------------------------

// Block adds a backing file for a declared block device ("NAME=FILE"; the
// NAME= may be omitted when the unikernel declares exactly one). Repeatable.
func (b *CreateBuilder) Block(nameFile string) *CreateBuilder {
	b.spec.Block = append(b.spec.Block, nameFile)
	return b
}

// GuestArgs sets the solo5 unikernel's own arguments (after a literal "--"
// — MirageOS options look like bsdkrun's own).
func (b *CreateBuilder) GuestArgs(args ...string) *CreateBuilder {
	b.spec.Args = args
	return b
}

// LogLevel overrides bsdkrun's global --log-level for the create (default
// 1, boot diagnostics — unlike the other calls).
func (b *CreateBuilder) LogLevel(level int) *CreateBuilder {
	b.logLevel = level
	b.hasLogUser = true
	return b
}

// Create boots the microVM (detached) and returns a handle to it.
func (b *CreateBuilder) Create() (*Sandbox, error) {
	args, err := BuildCreateArgs(b.spec)
	if err != nil {
		return nil, err
	}
	level := 1
	if b.hasLogUser {
		level = b.logLevel
	}
	res, err := Run(args, &RunOpts{LogLevel: level})
	if err != nil {
		return nil, err
	}
	if res.ExitCode != 0 {
		return nil, &CommandFailedError{
			ExitCode: res.ExitCode,
			Stdout:   res.Stdout,
			Stderr:   res.Stderr,
			Command:  "bsdkrun create",
		}
	}

	// Detached runs print just the machine id on stdout; take the last line
	// that looks like one, in case boot noise precedes it.
	var machineID string
	for _, line := range strings.Split(res.Stdout, "\n") {
		stripped := strings.TrimSpace(line)
		if machineIDRe.MatchString(stripped) {
			machineID = stripped
		}
	}
	if machineID == "" {
		return nil, &CommandFailedError{
			ExitCode: res.ExitCode,
			Stdout:   res.Stdout,
			Stderr:   res.Stderr,
			Command:  "bsdkrun create (no machine id in output)",
		}
	}

	sshPort := 0
	if match := sshPortRe.FindStringSubmatch(res.Stderr); match != nil {
		sshPort, _ = strconv.Atoi(match[1])
	}
	return &Sandbox{ID: machineID, SSHPort: sshPort}, nil
}
