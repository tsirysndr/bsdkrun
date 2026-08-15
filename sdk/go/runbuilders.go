package bsdkrun

import (
	"fmt"
	"strings"
)

// Fluent builders for the daemon's run* mutations. Each builder mirrors the
// matching GraphQL input object in daemon/src/graphql.rs field for field
// (camelCase on the wire) and ends in Launch, which returns the new
// machine's id:
//
//	id, err := client.RunLinux().
//		Image("alpine").
//		Cpus(2).Mem(1024).
//		Command("sleep", "300").
//		Launch()

// runNet accumulates the shared NetInput fields. Note that in the daemon's
// schema the machine *name* lives inside NetInput, so the builders' Name
// method lands here.
type runNet struct {
	used    bool
	noNet   bool
	ports   []string
	mac     *string
	network *string
	name    *string
}

// input renders the NetInput map, or nil when no net field was ever set —
// matching the Python client, which sends null for an absent net option.
func (n *runNet) input() any {
	if !n.used {
		return nil
	}
	return map[string]any{
		"noNet":   n.noNet,
		"ports":   stringsOrEmpty(n.ports),
		"mac":     optString(n.mac),
		"network": optString(n.network),
		"name":    optString(n.name),
	}
}

func optString(p *string) any {
	if p == nil {
		return nil
	}
	return *p
}

func optInt(p *int) any {
	if p == nil {
		return nil
	}
	return *p
}

func strPtr(s string) *string { return &s }
func intPtr(n int) *int       { return &n }

func (c *Client) launch(mutation string, field string, input map[string]any) (string, error) {
	data, err := c.Request(mutation, map[string]any{"input": input})
	if err != nil {
		return "", err
	}
	return asString(data[field]), nil
}

// -- runLinux ---------------------------------------------------------------

// RunLinuxBuilder configures the daemon's runLinux mutation.
type RunLinuxBuilder struct {
	c             *Client
	image         *string
	cpus, mem     *int
	net           runNet
	volume        *string
	mounts        []string
	attachDisk    []string
	env           []string
	entrypoint    *string
	initramfs     bool
	kernel        *string
	kernelVersion *string
	console       *string
	repo          *string
	command       []string
}

// RunLinux starts building a runLinux mutation. Image is required.
func (c *Client) RunLinux() *RunLinuxBuilder { return &RunLinuxBuilder{c: c} }

// Image sets the OCI image to boot (required).
func (b *RunLinuxBuilder) Image(image string) *RunLinuxBuilder { b.image = strPtr(image); return b }

// Cpus sets the vCPU count.
func (b *RunLinuxBuilder) Cpus(n int) *RunLinuxBuilder { b.cpus = intPtr(n); return b }

// Mem sets the RAM in MiB.
func (b *RunLinuxBuilder) Mem(mib int) *RunLinuxBuilder { b.mem = intPtr(mib); return b }

// Port forwards a host TCP port to the guest ("HOST:GUEST"). Repeatable.
func (b *RunLinuxBuilder) Port(hostGuest string) *RunLinuxBuilder {
	b.net.used = true
	b.net.ports = append(b.net.ports, hostGuest)
	return b
}

// Forward is Port with numeric halves.
func (b *RunLinuxBuilder) Forward(host, guest int) *RunLinuxBuilder {
	return b.Port(fmt.Sprintf("%d:%d", host, guest))
}

// Mac pins the guest MAC address.
func (b *RunLinuxBuilder) Mac(mac string) *RunLinuxBuilder {
	b.net.used = true
	b.net.mac = strPtr(mac)
	return b
}

// Network joins a global network.
func (b *RunLinuxBuilder) Network(name string) *RunLinuxBuilder {
	b.net.used = true
	b.net.network = strPtr(name)
	return b
}

// NoNet disables guest networking entirely.
func (b *RunLinuxBuilder) NoNet() *RunLinuxBuilder {
	b.net.used = true
	b.net.noNet = true
	return b
}

// Name names the machine.
func (b *RunLinuxBuilder) Name(name string) *RunLinuxBuilder {
	b.net.used = true
	b.net.name = strPtr(name)
	return b
}

// Volume uses a named persistent volume as the rootfs.
func (b *RunLinuxBuilder) Volume(name string) *RunLinuxBuilder { b.volume = strPtr(name); return b }

// Mount shares a host directory ("HOST:GUEST", optionally ":ro").
// Repeatable.
func (b *RunLinuxBuilder) Mount(hostGuest string) *RunLinuxBuilder {
	b.mounts = append(b.mounts, hostGuest)
	return b
}

// AttachDisk attaches an extra disk ("PATH" or "PATH:ro"). Repeatable.
func (b *RunLinuxBuilder) AttachDisk(disk string) *RunLinuxBuilder {
	b.attachDisk = append(b.attachDisk, disk)
	return b
}

// Env sets a guest environment variable. Repeatable.
func (b *RunLinuxBuilder) Env(key, value string) *RunLinuxBuilder {
	b.env = append(b.env, key+"="+value)
	return b
}

// Entrypoint overrides the image entrypoint.
func (b *RunLinuxBuilder) Entrypoint(entrypoint string) *RunLinuxBuilder {
	b.entrypoint = strPtr(entrypoint)
	return b
}

// Initramfs boots via an initramfs.
func (b *RunLinuxBuilder) Initramfs() *RunLinuxBuilder { b.initramfs = true; return b }

// Kernel overrides the guest kernel.
func (b *RunLinuxBuilder) Kernel(path string) *RunLinuxBuilder { b.kernel = strPtr(path); return b }

// KernelVersion picks a packaged kernel version.
func (b *RunLinuxBuilder) KernelVersion(version string) *RunLinuxBuilder {
	b.kernelVersion = strPtr(version)
	return b
}

// Console selects the guest console device.
func (b *RunLinuxBuilder) Console(console string) *RunLinuxBuilder {
	b.console = strPtr(console)
	return b
}

// Repo pins the agent repository.
func (b *RunLinuxBuilder) Repo(repo string) *RunLinuxBuilder { b.repo = strPtr(repo); return b }

// Command sets the guest command.
func (b *RunLinuxBuilder) Command(argv ...string) *RunLinuxBuilder { b.command = argv; return b }

// Launch boots the machine and returns its id.
func (b *RunLinuxBuilder) Launch() (string, error) {
	if b.image == nil {
		return "", fmt.Errorf("RunLinux requires Image()")
	}
	return b.c.launch(runLinuxMutation, "runLinux", map[string]any{
		"image":         *b.image,
		"cpus":          optInt(b.cpus),
		"mem":           optInt(b.mem),
		"net":           b.net.input(),
		"volume":        optString(b.volume),
		"mounts":        stringsOrEmpty(b.mounts),
		"attachDisk":    stringsOrEmpty(b.attachDisk),
		"env":           stringsOrEmpty(b.env),
		"entrypoint":    optString(b.entrypoint),
		"initramfs":     b.initramfs,
		"kernel":        optString(b.kernel),
		"kernelVersion": optString(b.kernelVersion),
		"console":       optString(b.console),
		"repo":          optString(b.repo),
		"command":       stringsOrEmpty(b.command),
	})
}

// -- runBsd -----------------------------------------------------------------

// RunBSDBuilder configures the daemon's runBsd mutation.
type RunBSDBuilder struct {
	c          *Client
	os         *string
	version    *string
	cpus, mem  *int
	net        runNet
	volume     *string
	persist    bool
	force      bool
	firmware   *string
	attachDisk []string
	diskSize   *string
	repo       *string
	command    []string
}

// RunBSD starts building a runBsd mutation. OS is required.
func (c *Client) RunBSD() *RunBSDBuilder { return &RunBSDBuilder{c: c} }

// OS picks the guest: "freebsd" or "netbsd" (required).
func (b *RunBSDBuilder) OS(osKind string) *RunBSDBuilder { b.os = strPtr(osKind); return b }

// Version picks the OS release.
func (b *RunBSDBuilder) Version(version string) *RunBSDBuilder { b.version = strPtr(version); return b }

// Cpus sets the vCPU count.
func (b *RunBSDBuilder) Cpus(n int) *RunBSDBuilder { b.cpus = intPtr(n); return b }

// Mem sets the RAM in MiB.
func (b *RunBSDBuilder) Mem(mib int) *RunBSDBuilder { b.mem = intPtr(mib); return b }

// Port forwards a host TCP port to the guest ("HOST:GUEST"). Repeatable.
func (b *RunBSDBuilder) Port(hostGuest string) *RunBSDBuilder {
	b.net.used = true
	b.net.ports = append(b.net.ports, hostGuest)
	return b
}

// Forward is Port with numeric halves.
func (b *RunBSDBuilder) Forward(host, guest int) *RunBSDBuilder {
	return b.Port(fmt.Sprintf("%d:%d", host, guest))
}

// Mac pins the guest MAC address.
func (b *RunBSDBuilder) Mac(mac string) *RunBSDBuilder {
	b.net.used = true
	b.net.mac = strPtr(mac)
	return b
}

// Network joins a global network.
func (b *RunBSDBuilder) Network(name string) *RunBSDBuilder {
	b.net.used = true
	b.net.network = strPtr(name)
	return b
}

// NoNet disables guest networking entirely.
func (b *RunBSDBuilder) NoNet() *RunBSDBuilder {
	b.net.used = true
	b.net.noNet = true
	return b
}

// Name names the machine.
func (b *RunBSDBuilder) Name(name string) *RunBSDBuilder {
	b.net.used = true
	b.net.name = strPtr(name)
	return b
}

// Volume uses a named persistent volume as the rootfs.
func (b *RunBSDBuilder) Volume(name string) *RunBSDBuilder { b.volume = strPtr(name); return b }

// Persist keeps the root disk across runs.
func (b *RunBSDBuilder) Persist() *RunBSDBuilder { b.persist = true; return b }

// Force re-fetches/rebuilds the image even when cached.
func (b *RunBSDBuilder) Force() *RunBSDBuilder { b.force = true; return b }

// Firmware overrides the UEFI firmware (freebsd).
func (b *RunBSDBuilder) Firmware(path string) *RunBSDBuilder { b.firmware = strPtr(path); return b }

// AttachDisk attaches an extra disk ("PATH" or "PATH:ro"). Repeatable.
func (b *RunBSDBuilder) AttachDisk(disk string) *RunBSDBuilder {
	b.attachDisk = append(b.attachDisk, disk)
	return b
}

// DiskSize sets the root disk size (e.g. "10G").
func (b *RunBSDBuilder) DiskSize(size string) *RunBSDBuilder { b.diskSize = strPtr(size); return b }

// Repo pins the agent repository.
func (b *RunBSDBuilder) Repo(repo string) *RunBSDBuilder { b.repo = strPtr(repo); return b }

// Command sets the guest command.
func (b *RunBSDBuilder) Command(argv ...string) *RunBSDBuilder { b.command = argv; return b }

// Launch boots the machine and returns its id.
func (b *RunBSDBuilder) Launch() (string, error) {
	if b.os == nil {
		return "", fmt.Errorf("RunBSD requires OS()")
	}
	var osEnum string
	switch strings.ToLower(strings.TrimSpace(*b.os)) {
	case "freebsd":
		osEnum = "FREEBSD"
	case "netbsd":
		osEnum = "NETBSD"
	default:
		return "", fmt.Errorf("RunBSD requires OS(\"freebsd\") or OS(\"netbsd\"), got %q", *b.os)
	}
	return b.c.launch(runBsdMutation, "runBsd", map[string]any{
		"os":         osEnum,
		"version":    optString(b.version),
		"cpus":       optInt(b.cpus),
		"mem":        optInt(b.mem),
		"net":        b.net.input(),
		"volume":     optString(b.volume),
		"persist":    b.persist,
		"force":      b.force,
		"firmware":   optString(b.firmware),
		"attachDisk": stringsOrEmpty(b.attachDisk),
		"diskSize":   optString(b.diskSize),
		"repo":       optString(b.repo),
		"command":    stringsOrEmpty(b.command),
	})
}

// -- runNanos ---------------------------------------------------------------

// RunNanosBuilder configures the daemon's runNanos mutation.
type RunNanosBuilder struct {
	c         *Client
	image     *string
	cpus, mem *int
	net       runNet
	kernel    *string
	cmdline   *string
	persist   bool
}

// RunNanos starts building a runNanos mutation. Image is required.
func (c *Client) RunNanos() *RunNanosBuilder { return &RunNanosBuilder{c: c} }

// Image sets the Nanos image: a path, or a bare name in ~/.ops/images
// (required).
func (b *RunNanosBuilder) Image(image string) *RunNanosBuilder { b.image = strPtr(image); return b }

// Cpus sets the vCPU count.
func (b *RunNanosBuilder) Cpus(n int) *RunNanosBuilder { b.cpus = intPtr(n); return b }

// Mem sets the RAM in MiB.
func (b *RunNanosBuilder) Mem(mib int) *RunNanosBuilder { b.mem = intPtr(mib); return b }

// Port forwards a host TCP port to the guest ("HOST:GUEST"). Repeatable.
func (b *RunNanosBuilder) Port(hostGuest string) *RunNanosBuilder {
	b.net.used = true
	b.net.ports = append(b.net.ports, hostGuest)
	return b
}

// Forward is Port with numeric halves.
func (b *RunNanosBuilder) Forward(host, guest int) *RunNanosBuilder {
	return b.Port(fmt.Sprintf("%d:%d", host, guest))
}

// Mac pins the guest MAC address.
func (b *RunNanosBuilder) Mac(mac string) *RunNanosBuilder {
	b.net.used = true
	b.net.mac = strPtr(mac)
	return b
}

// Network joins a global network.
func (b *RunNanosBuilder) Network(name string) *RunNanosBuilder {
	b.net.used = true
	b.net.network = strPtr(name)
	return b
}

// NoNet disables guest networking entirely.
func (b *RunNanosBuilder) NoNet() *RunNanosBuilder {
	b.net.used = true
	b.net.noNet = true
	return b
}

// Name names the machine.
func (b *RunNanosBuilder) Name(name string) *RunNanosBuilder {
	b.net.used = true
	b.net.name = strPtr(name)
	return b
}

// Kernel overrides the Nanos kernel (Linux hosts).
func (b *RunNanosBuilder) Kernel(path string) *RunNanosBuilder { b.kernel = strPtr(path); return b }

// Cmdline sets the kernel command line.
func (b *RunNanosBuilder) Cmdline(cmdline string) *RunNanosBuilder {
	b.cmdline = strPtr(cmdline)
	return b
}

// Persist keeps the root disk across runs.
func (b *RunNanosBuilder) Persist() *RunNanosBuilder { b.persist = true; return b }

// Launch boots the machine and returns its id.
func (b *RunNanosBuilder) Launch() (string, error) {
	if b.image == nil {
		return "", fmt.Errorf("RunNanos requires Image()")
	}
	return b.c.launch(runNanosMutation, "runNanos", map[string]any{
		"image":   *b.image,
		"cpus":    optInt(b.cpus),
		"mem":     optInt(b.mem),
		"net":     b.net.input(),
		"kernel":  optString(b.kernel),
		"cmdline": optString(b.cmdline),
		"persist": b.persist,
	})
}

// -- runUnikraft ------------------------------------------------------------

// RunUnikraftBuilder configures the daemon's runUnikraft mutation.
type RunUnikraftBuilder struct {
	c         *Client
	path      *string
	cpus, mem *int
	net       runNet
	cmdline   *string
	initramfs *string
	mounts    []string
}

// RunUnikraft starts building a runUnikraft mutation.
func (c *Client) RunUnikraft() *RunUnikraftBuilder { return &RunUnikraftBuilder{c: c} }

// Path sets the kraft project directory or built unikernel image (the
// daemon defaults to ".").
func (b *RunUnikraftBuilder) Path(path string) *RunUnikraftBuilder { b.path = strPtr(path); return b }

// Cpus sets the vCPU count.
func (b *RunUnikraftBuilder) Cpus(n int) *RunUnikraftBuilder { b.cpus = intPtr(n); return b }

// Mem sets the RAM in MiB.
func (b *RunUnikraftBuilder) Mem(mib int) *RunUnikraftBuilder { b.mem = intPtr(mib); return b }

// Port forwards a host TCP port to the guest ("HOST:GUEST"). Repeatable.
func (b *RunUnikraftBuilder) Port(hostGuest string) *RunUnikraftBuilder {
	b.net.used = true
	b.net.ports = append(b.net.ports, hostGuest)
	return b
}

// Forward is Port with numeric halves.
func (b *RunUnikraftBuilder) Forward(host, guest int) *RunUnikraftBuilder {
	return b.Port(fmt.Sprintf("%d:%d", host, guest))
}

// Mac pins the guest MAC address.
func (b *RunUnikraftBuilder) Mac(mac string) *RunUnikraftBuilder {
	b.net.used = true
	b.net.mac = strPtr(mac)
	return b
}

// Network joins a global network.
func (b *RunUnikraftBuilder) Network(name string) *RunUnikraftBuilder {
	b.net.used = true
	b.net.network = strPtr(name)
	return b
}

// NoNet disables guest networking entirely.
func (b *RunUnikraftBuilder) NoNet() *RunUnikraftBuilder {
	b.net.used = true
	b.net.noNet = true
	return b
}

// Name names the machine.
func (b *RunUnikraftBuilder) Name(name string) *RunUnikraftBuilder {
	b.net.used = true
	b.net.name = strPtr(name)
	return b
}

// Cmdline sets the kernel command line; Unikraft hands it to the
// application as argv.
func (b *RunUnikraftBuilder) Cmdline(cmdline string) *RunUnikraftBuilder {
	b.cmdline = strPtr(cmdline)
	return b
}

// Initramfs sets the initramfs image path.
func (b *RunUnikraftBuilder) Initramfs(path string) *RunUnikraftBuilder {
	b.initramfs = strPtr(path)
	return b
}

// Mount adds a virtio-fs share ("HOST:GUEST" with an absolute guest path).
// Repeatable.
func (b *RunUnikraftBuilder) Mount(hostGuest string) *RunUnikraftBuilder {
	b.mounts = append(b.mounts, hostGuest)
	return b
}

// Launch boots the unikernel and returns its id.
func (b *RunUnikraftBuilder) Launch() (string, error) {
	return b.c.launch(runUnikraftMutation, "runUnikraft", map[string]any{
		"path":      optString(b.path),
		"cpus":      optInt(b.cpus),
		"mem":       optInt(b.mem),
		"net":       b.net.input(),
		"cmdline":   optString(b.cmdline),
		"initramfs": optString(b.initramfs),
		"mounts":    stringsOrEmpty(b.mounts),
	})
}

// -- runSolo5 ---------------------------------------------------------------

// RunSolo5Builder configures the daemon's runSolo5 mutation. Solo5
// (MirageOS) runs under the solo5-hvt tender rather than libkrun; the
// unikernel declares its own network and block devices in its MFT1 manifest
// note, so only what the host alone can know is asked for: block backing
// files ("NAME=FILE") and the unikernel's own args (e.g.
// "--ipv4=10.0.0.2/24"). Like unikraft, no disk and no agent.
type RunSolo5Builder struct {
	c         *Client
	path      *string
	cpus, mem *int
	net       runNet
	block     []string
	args      []string
}

// RunSolo5 starts building a runSolo5 mutation.
func (c *Client) RunSolo5() *RunSolo5Builder { return &RunSolo5Builder{c: c} }

// Path sets the .hvt binary, or a project directory whose dist/ holds one
// (the daemon defaults to ".").
func (b *RunSolo5Builder) Path(path string) *RunSolo5Builder { b.path = strPtr(path); return b }

// Cpus sets the vCPU count (solo5 is always a single vCPU; above 1 is
// warned about and ignored by the daemon).
func (b *RunSolo5Builder) Cpus(n int) *RunSolo5Builder { b.cpus = intPtr(n); return b }

// Mem sets the RAM in MiB.
func (b *RunSolo5Builder) Mem(mib int) *RunSolo5Builder { b.mem = intPtr(mib); return b }

// Port forwards a host TCP port to the guest ("HOST:GUEST"). Repeatable.
func (b *RunSolo5Builder) Port(hostGuest string) *RunSolo5Builder {
	b.net.used = true
	b.net.ports = append(b.net.ports, hostGuest)
	return b
}

// Forward is Port with numeric halves.
func (b *RunSolo5Builder) Forward(host, guest int) *RunSolo5Builder {
	return b.Port(fmt.Sprintf("%d:%d", host, guest))
}

// Mac pins the guest MAC address.
func (b *RunSolo5Builder) Mac(mac string) *RunSolo5Builder {
	b.net.used = true
	b.net.mac = strPtr(mac)
	return b
}

// Network joins a global network.
func (b *RunSolo5Builder) Network(name string) *RunSolo5Builder {
	b.net.used = true
	b.net.network = strPtr(name)
	return b
}

// NoNet disables guest networking entirely.
func (b *RunSolo5Builder) NoNet() *RunSolo5Builder {
	b.net.used = true
	b.net.noNet = true
	return b
}

// Name names the machine.
func (b *RunSolo5Builder) Name(name string) *RunSolo5Builder {
	b.net.used = true
	b.net.name = strPtr(name)
	return b
}

// Block adds a backing file for a declared block device ("NAME=FILE").
// Repeatable.
func (b *RunSolo5Builder) Block(nameFile string) *RunSolo5Builder {
	b.block = append(b.block, nameFile)
	return b
}

// Args sets the unikernel's own arguments (e.g. "--ipv4=10.0.0.2/24").
func (b *RunSolo5Builder) Args(args ...string) *RunSolo5Builder { b.args = args; return b }

// Launch boots the unikernel and returns its id.
func (b *RunSolo5Builder) Launch() (string, error) {
	return b.c.launch(runSolo5Mutation, "runSolo5", map[string]any{
		"path":  optString(b.path),
		"cpus":  optInt(b.cpus),
		"mem":   optInt(b.mem),
		"net":   b.net.input(),
		"block": stringsOrEmpty(b.block),
		"args":  stringsOrEmpty(b.args),
	})
}

// -- runOsv -----------------------------------------------------------------

// RunOSvBuilder configures the daemon's runOsv mutation.
type RunOSvBuilder struct {
	c          *Client
	image      *string
	cpus, mem  *int
	net        runNet
	cmdline    *string
	disk       *string
	noDisk     bool
	attachDisk []string
	gic        *string
	persist    bool
	volume     *string
}

// RunOSv starts building a runOsv mutation. Image is required.
func (c *Client) RunOSv() *RunOSvBuilder { return &RunOSvBuilder{c: c} }

// Image sets the OSv image: an aarch64 loader.img, or on x86_64 the loader
// ELF (required; the latter needs Disk).
func (b *RunOSvBuilder) Image(image string) *RunOSvBuilder { b.image = strPtr(image); return b }

// Cpus sets the vCPU count.
func (b *RunOSvBuilder) Cpus(n int) *RunOSvBuilder { b.cpus = intPtr(n); return b }

// Mem sets the RAM in MiB.
func (b *RunOSvBuilder) Mem(mib int) *RunOSvBuilder { b.mem = intPtr(mib); return b }

// Port forwards a host TCP port to the guest ("HOST:GUEST"). Repeatable.
func (b *RunOSvBuilder) Port(hostGuest string) *RunOSvBuilder {
	b.net.used = true
	b.net.ports = append(b.net.ports, hostGuest)
	return b
}

// Forward is Port with numeric halves.
func (b *RunOSvBuilder) Forward(host, guest int) *RunOSvBuilder {
	return b.Port(fmt.Sprintf("%d:%d", host, guest))
}

// Mac pins the guest MAC address.
func (b *RunOSvBuilder) Mac(mac string) *RunOSvBuilder {
	b.net.used = true
	b.net.mac = strPtr(mac)
	return b
}

// Network joins a global network.
func (b *RunOSvBuilder) Network(name string) *RunOSvBuilder {
	b.net.used = true
	b.net.network = strPtr(name)
	return b
}

// NoNet disables guest networking entirely.
func (b *RunOSvBuilder) NoNet() *RunOSvBuilder {
	b.net.used = true
	b.net.noNet = true
	return b
}

// Name names the machine.
func (b *RunOSvBuilder) Name(name string) *RunOSvBuilder {
	b.net.used = true
	b.net.name = strPtr(name)
	return b
}

// Cmdline sets the application to run and its arguments, e.g. "/hello.so".
func (b *RunOSvBuilder) Cmdline(cmdline string) *RunOSvBuilder { b.cmdline = strPtr(cmdline); return b }

// Disk sets the root disk (raw). Required on x86_64.
func (b *RunOSvBuilder) Disk(path string) *RunOSvBuilder { b.disk = strPtr(path); return b }

// NoDisk boots the kernel alone, with no root filesystem to mount.
func (b *RunOSvBuilder) NoDisk() *RunOSvBuilder { b.noDisk = true; return b }

// AttachDisk attaches an extra disk as virtio-blk ("PATH" or "PATH:ro").
// Repeatable.
func (b *RunOSvBuilder) AttachDisk(disk string) *RunOSvBuilder {
	b.attachDisk = append(b.attachDisk, disk)
	return b
}

// Gic picks the interrupt controller revision, "v2" (the default) or "v3"
// (aarch64 only).
func (b *RunOSvBuilder) Gic(gic string) *RunOSvBuilder { b.gic = strPtr(gic); return b }

// Persist keeps the root disk across runs.
func (b *RunOSvBuilder) Persist() *RunOSvBuilder { b.persist = true; return b }

// Volume uses a named persistent volume as the rootfs.
func (b *RunOSvBuilder) Volume(name string) *RunOSvBuilder { b.volume = strPtr(name); return b }

// Launch boots the machine and returns its id.
func (b *RunOSvBuilder) Launch() (string, error) {
	if b.image == nil {
		return "", fmt.Errorf("RunOSv requires Image()")
	}
	return b.c.launch(runOsvMutation, "runOsv", map[string]any{
		"image":      *b.image,
		"cpus":       optInt(b.cpus),
		"mem":        optInt(b.mem),
		"net":        b.net.input(),
		"cmdline":    optString(b.cmdline),
		"disk":       optString(b.disk),
		"noDisk":     b.noDisk,
		"attachDisk": stringsOrEmpty(b.attachDisk),
		"gic":        optString(b.gic),
		"persist":    b.persist,
		"volume":     optString(b.volume),
	})
}

// -- runFlavor --------------------------------------------------------------

// RunFlavorBuilder configures the daemon's runFlavor mutation. A flavor's
// ports live at the top level of the input (there is no NetInput here).
type RunFlavorBuilder struct {
	c         *Client
	name      *string
	cpus, mem *int
	ports     []string
	volume    *string
	repo      *string
}

// RunFlavor starts building a runFlavor mutation. Name is required.
func (c *Client) RunFlavor() *RunFlavorBuilder { return &RunFlavorBuilder{c: c} }

// Name picks the flavor to boot (required).
func (b *RunFlavorBuilder) Name(name string) *RunFlavorBuilder { b.name = strPtr(name); return b }

// Cpus sets the vCPU count.
func (b *RunFlavorBuilder) Cpus(n int) *RunFlavorBuilder { b.cpus = intPtr(n); return b }

// Mem sets the RAM in MiB.
func (b *RunFlavorBuilder) Mem(mib int) *RunFlavorBuilder { b.mem = intPtr(mib); return b }

// Port forwards a host TCP port to the guest ("HOST:GUEST"). Repeatable.
func (b *RunFlavorBuilder) Port(hostGuest string) *RunFlavorBuilder {
	b.ports = append(b.ports, hostGuest)
	return b
}

// Forward is Port with numeric halves.
func (b *RunFlavorBuilder) Forward(host, guest int) *RunFlavorBuilder {
	return b.Port(fmt.Sprintf("%d:%d", host, guest))
}

// Volume uses a named persistent volume as the rootfs.
func (b *RunFlavorBuilder) Volume(name string) *RunFlavorBuilder { b.volume = strPtr(name); return b }

// Repo pins the agent repository.
func (b *RunFlavorBuilder) Repo(repo string) *RunFlavorBuilder { b.repo = strPtr(repo); return b }

// Launch boots the flavor and returns the machine's id.
func (b *RunFlavorBuilder) Launch() (string, error) {
	if b.name == nil {
		return "", fmt.Errorf("RunFlavor requires Name()")
	}
	return b.c.launch(runFlavorMutation, "runFlavor", map[string]any{
		"name":   *b.name,
		"cpus":   optInt(b.cpus),
		"mem":    optInt(b.mem),
		"ports":  stringsOrEmpty(b.ports),
		"volume": optString(b.volume),
		"repo":   optString(b.repo),
	})
}
