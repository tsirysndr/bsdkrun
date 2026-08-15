package bsdkrun

// Unit tests for the create-argv builder (no VM needed), ported from the
// Python SDK's test_args.py.

import (
	"reflect"
	"testing"
)

func mustBuild(t *testing.T, spec CreateSpec) []string {
	t.Helper()
	args, err := BuildCreateArgs(spec)
	if err != nil {
		t.Fatalf("BuildCreateArgs(%+v): %v", spec, err)
	}
	return args
}

func TestLinuxMinimal(t *testing.T) {
	got := mustBuild(t, CreateSpec{OS: "linux", Image: "alpine"})
	want := []string{"linux", "alpine", "-d"}
	if !reflect.DeepEqual(got, want) {
		t.Fatalf("got %v, want %v", got, want)
	}
}

func TestLinuxFull(t *testing.T) {
	got := mustBuild(t, CreateSpec{
		OS:            "linux",
		Image:         "ghcr.io/owner/name:tag",
		Kernel:        "vmlinux",
		KernelVersion: "6.6",
		Initramfs:     true,
		Volume:        "web",
		Mounts:        []string{"~/project:/src", "~/data:/data:ro"},
		Entrypoint:    "/bin/sh",
		Console:       "hvc0",
		Net:           &NetOpts{Ports: []string{"8080:80", "2222:22"}, Network: "devnet"},
		Name:          "api",
		Cpus:          2,
		Mem:           1024,
		Command:       []string{"node", "server.js"},
	})
	want := []string{
		"linux", "ghcr.io/owner/name:tag", "-d",
		"--kernel", "vmlinux",
		"--kernel-version", "6.6",
		"--initramfs",
		"-v", "web",
		"--mount", "~/project:/src",
		"--mount", "~/data:/data:ro",
		"--entrypoint", "/bin/sh",
		"--console", "hvc0",
		"--port", "8080:80",
		"--port", "2222:22",
		"--network", "devnet",
		"--name", "api",
		"--cpus", "2",
		"--mem", "1024",
		"--", "node", "server.js",
	}
	if !reflect.DeepEqual(got, want) {
		t.Fatalf("got %v, want %v", got, want)
	}
}

func TestNetDisabledOrdering(t *testing.T) {
	// --no-net, then ports, then --mac, then --network.
	got := mustBuild(t, CreateSpec{
		OS:    "linux",
		Image: "alpine",
		Net: &NetOpts{
			Disabled: true,
			Ports:    []string{"2222:22"},
			Mac:      "de:ad:be:ef:00:01",
			Network:  "devnet",
		},
	})
	want := []string{
		"linux", "alpine", "-d",
		"--no-net",
		"--port", "2222:22",
		"--mac", "de:ad:be:ef:00:01",
		"--network", "devnet",
	}
	if !reflect.DeepEqual(got, want) {
		t.Fatalf("got %v, want %v", got, want)
	}
}

func TestFreeBSDArgs(t *testing.T) {
	got := mustBuild(t, CreateSpec{
		OS:         "freebsd",
		Version:    "14.3",
		Firmware:   "KRUN_EFI.fd",
		Force:      true,
		Persist:    true,
		Volume:     "db",
		AttachDisk: []string{"extra.raw", "ro.raw:ro"},
		Mem:        2048,
		Name:       "bsd",
	})
	want := []string{
		"freebsd", "-d",
		"--version", "14.3",
		"--firmware", "KRUN_EFI.fd",
		"--force",
		"--persist",
		"-v", "db",
		"--attach-disk", "extra.raw",
		"--attach-disk", "ro.raw:ro",
		"--name", "bsd",
		"--mem", "2048",
	}
	if !reflect.DeepEqual(got, want) {
		t.Fatalf("got %v, want %v", got, want)
	}
}

func TestNetBSDArgs(t *testing.T) {
	got := mustBuild(t, CreateSpec{OS: "netbsd", Version: "10.1", Volume: "db"})
	want := []string{"netbsd", "-d", "--version", "10.1", "-v", "db"}
	if !reflect.DeepEqual(got, want) {
		t.Fatalf("got %v, want %v", got, want)
	}
}

func TestFirmwareArgs(t *testing.T) {
	got := mustBuild(t, CreateSpec{OS: "firmware", Firmware: "KRUN_EFI.fd", Disk: "disk.raw"})
	want := []string{"firmware", "--firmware", "KRUN_EFI.fd", "--disk", "disk.raw", "-d"}
	if !reflect.DeepEqual(got, want) {
		t.Fatalf("got %v, want %v", got, want)
	}
}

func TestKernelArgs(t *testing.T) {
	// For the kernel kind, --initramfs takes a path (InitramfsPath), not a
	// bare flag.
	got := mustBuild(t, CreateSpec{
		OS:            "kernel",
		Kernel:        "netbsd",
		Format:        "elf",
		InitramfsPath: "initrd.img",
		Cmdline:       "root=ld0a",
		Disk:          "root.raw",
	})
	want := []string{
		"kernel", "--kernel", "netbsd", "-d",
		"--format", "elf",
		"--initramfs", "initrd.img",
		"--cmdline", "root=ld0a",
		"--disk", "root.raw",
	}
	if !reflect.DeepEqual(got, want) {
		t.Fatalf("got %v, want %v", got, want)
	}
}

func TestNanosArgs(t *testing.T) {
	got := mustBuild(t, CreateSpec{
		OS:      "nanos",
		Image:   "hello",
		Cmdline: "verbose",
		Persist: true,
		Net:     &NetOpts{Ports: []string{"8080:8080"}},
	})
	want := []string{
		"nanos", "-d",
		"--cmdline", "verbose",
		"--persist",
		"--port", "8080:8080",
		"hello",
	}
	if !reflect.DeepEqual(got, want) {
		t.Fatalf("got %v, want %v", got, want)
	}
}

func TestUnikraftArgs(t *testing.T) {
	got := mustBuild(t, CreateSpec{
		OS:      "unikraft",
		Cmdline: "hello",
		Mounts:  []string{"./data:/data"},
	})
	want := []string{"unikraft", "-d", "--cmdline", "hello", "--mount", "./data:/data", "."}
	if !reflect.DeepEqual(got, want) {
		t.Fatalf("got %v, want %v", got, want)
	}
}

func TestSolo5Args(t *testing.T) {
	got := mustBuild(t, CreateSpec{
		OS:    "solo5",
		Path:  "dist/hello.hvt",
		Block: []string{"storage=disk.img"},
		Args:  []string{"--ipv4=10.0.0.2/24"},
	})
	want := []string{
		"solo5", "-d",
		"--block", "storage=disk.img",
		"dist/hello.hvt",
		"--", "--ipv4=10.0.0.2/24",
	}
	if !reflect.DeepEqual(got, want) {
		t.Fatalf("got %v, want %v", got, want)
	}
}

func TestMissingRequiredErrors(t *testing.T) {
	for _, spec := range []CreateSpec{
		{OS: "linux"},
		{OS: "firmware", Firmware: "fw.fd"},
		{OS: "kernel"},
		{OS: "nanos"},
		{OS: "osv"},
	} {
		if _, err := BuildCreateArgs(spec); err == nil {
			t.Errorf("BuildCreateArgs(%+v): expected an error", spec)
		}
	}
}

func TestUnknownOSErrors(t *testing.T) {
	if _, err := BuildCreateArgs(CreateSpec{OS: "plan9"}); err == nil {
		t.Fatal("expected an error for os plan9")
	}
}

// A Go map has no iteration order at all, so the builder sorts by key —
// otherwise the same spec would produce a different command line run to run.
func TestLinuxEnvIsEmittedSortedByKey(t *testing.T) {
	got, err := BuildCreateArgs(CreateSpec{
		OS:    "linux",
		Image: "alpine",
		Env:   map[string]string{"ZED": "3", "ALPHA": "1", "MID": "2"},
	})
	if err != nil {
		t.Fatalf("BuildCreateArgs: %v", err)
	}
	want := []string{"linux", "alpine", "-d", "-e", "ALPHA=1", "-e", "MID=2", "-e", "ZED=3"}
	if !reflect.DeepEqual(got, want) {
		t.Errorf("got %v, want %v", got, want)
	}
}

func TestLinuxWithoutEnvEmitsNothing(t *testing.T) {
	for _, env := range []map[string]string{nil, {}} {
		got, err := BuildCreateArgs(CreateSpec{OS: "linux", Image: "alpine", Env: env})
		if err != nil {
			t.Fatalf("BuildCreateArgs: %v", err)
		}
		if !reflect.DeepEqual(got, []string{"linux", "alpine", "-d"}) {
			t.Errorf("env %v: got %v", env, got)
		}
	}
}
