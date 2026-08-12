package bsdkrun

import (
	"encoding/json"
	"strings"
)

// Host-level operations that aren't tied to a single machine, grouped into
// the same namespaces the Python SDK ships as submodules:
// bsdkrun.Images.List(), bsdkrun.Volumes.Remove(...),
// bsdkrun.Networks.Create(...), bsdkrun.System.Probe().

// ImagesNamespace groups host-level image operations; use the Images var.
type ImagesNamespace struct{}

// Images is the image namespace.
var Images ImagesNamespace

// List lists downloaded images (pulled OCI images + fetched BSD images).
func (ImagesNamespace) List() ([]ImageInfo, error) {
	res, err := RunChecked([]string{"images", "--json"}, "bsdkrun images", nil)
	if err != nil {
		return nil, err
	}
	var rows []ImageInfo
	if err := json.Unmarshal([]byte(orEmptyJSON(res.Stdout)), &rows); err != nil {
		return nil, err
	}
	return rows, nil
}

// VolumesNamespace groups host-level volume operations; use the Volumes var.
type VolumesNamespace struct{}

// Volumes is the volume namespace.
var Volumes VolumesNamespace

// List lists persistent volumes.
func (VolumesNamespace) List() ([]VolumeInfo, error) {
	res, err := RunChecked([]string{"volume", "ls", "--json"}, "bsdkrun volume ls", nil)
	if err != nil {
		return nil, err
	}
	var rows []VolumeInfo
	if err := json.Unmarshal([]byte(orEmptyJSON(res.Stdout)), &rows); err != nil {
		return nil, err
	}
	return rows, nil
}

// Remove removes one or more volumes (and their data).
func (VolumesNamespace) Remove(names ...string) error {
	return removeVolumes(false, names)
}

// ForceRemove removes one or more volumes even when in use.
func (VolumesNamespace) ForceRemove(names ...string) error {
	return removeVolumes(true, names)
}

func removeVolumes(force bool, names []string) error {
	args := []string{"volume", "rm"}
	if force {
		args = append(args, "--force")
	}
	args = append(args, names...)
	_, err := RunChecked(args, "bsdkrun volume rm", nil)
	return err
}

// NetworksNamespace groups global-network operations — reach machines by
// name on a shared subnet; use the Networks var.
type NetworksNamespace struct{}

// Networks is the network namespace.
var Networks NetworksNamespace

// List lists global networks and their member counts.
func (NetworksNamespace) List() ([]NetworkInfo, error) {
	res, err := RunChecked([]string{"network", "ls", "--json"}, "bsdkrun network ls", nil)
	if err != nil {
		return nil, err
	}
	var rows []NetworkInfo
	if err := json.Unmarshal([]byte(orEmptyJSON(res.Stdout)), &rows); err != nil {
		return nil, err
	}
	return rows, nil
}

// Create creates a global network (starts its shared switch).
func (NetworksNamespace) Create(name string) error {
	_, err := RunChecked([]string{"network", "create", name}, "bsdkrun network create", nil)
	return err
}

// Remove removes one or more networks.
func (NetworksNamespace) Remove(names ...string) error {
	return removeNetworks(false, names)
}

// ForceRemove removes one or more networks even with live members.
func (NetworksNamespace) ForceRemove(names ...string) error {
	return removeNetworks(true, names)
}

func removeNetworks(force bool, names []string) error {
	args := []string{"network", "rm"}
	if force {
		args = append(args, "--force")
	}
	args = append(args, names...)
	_, err := RunChecked(args, "bsdkrun network rm", nil)
	return err
}

// Connect joins or switches a machine (by id or name) to a network (next
// start).
func (NetworksNamespace) Connect(machine, network string) error {
	_, err := RunChecked([]string{"network", "connect", machine, network}, "bsdkrun network connect", nil)
	return err
}

// Disconnect detaches a machine from its network. Applies on its next
// start.
func (NetworksNamespace) Disconnect(machine string) error {
	_, err := RunChecked([]string{"network", "disconnect", machine}, "bsdkrun network disconnect", nil)
	return err
}

// Sync refreshes members' /etc/hosts so peers resolve by name (notably
// NetBSD).
func (NetworksNamespace) Sync(network string) error {
	_, err := RunChecked([]string{"network", "sync", network}, "bsdkrun network sync", nil)
	return err
}

// Members lists the machines currently attached to network (running or
// stopped).
func (NetworksNamespace) Members(network string) ([]SandboxInfo, error) {
	rows, err := ListSandboxes(true)
	if err != nil {
		return nil, err
	}
	var out []SandboxInfo
	for _, info := range rows {
		if info.Network == network {
			out = append(out, info)
		}
	}
	return out, nil
}

// SystemNamespace groups host toolchain / image operations; use the System
// var.
type SystemNamespace struct{}

// System is the system namespace.
var System SystemNamespace

// Probe sanity-checks the toolchain (libkrun links, a context is
// creatable). It does not boot. Returns true on success.
func (SystemNamespace) Probe() bool {
	res, err := Run([]string{"probe"}, nil)
	return err == nil && res.ExitCode == 0
}

// FetchImageBuilder downloads + prepares a BSD image ahead of time.
type FetchImageBuilder struct {
	osName  string
	version string
	dir     string
	force   bool
}

// FetchImage starts a fetch for "freebsd" or "netbsd": chain
// Version/Dir/Force, then Run.
func (SystemNamespace) FetchImage(osName string) *FetchImageBuilder {
	return &FetchImageBuilder{osName: osName}
}

// Version picks the release to fetch.
func (b *FetchImageBuilder) Version(version string) *FetchImageBuilder {
	b.version = version
	return b
}

// Dir sets the download directory.
func (b *FetchImageBuilder) Dir(dir string) *FetchImageBuilder {
	b.dir = dir
	return b
}

// Force re-downloads even when cached.
func (b *FetchImageBuilder) Force() *FetchImageBuilder {
	b.force = true
	return b
}

// Run performs the fetch and returns the command output.
func (b *FetchImageBuilder) Run() (string, error) {
	args := []string{"fetch", "--os", b.osName}
	if b.version != "" {
		args = append(args, "--version", b.version)
	}
	if b.dir != "" {
		args = append(args, "--dir", b.dir)
	}
	if b.force {
		args = append(args, "--force")
	}
	res, err := RunChecked(args, "bsdkrun fetch", nil)
	if err != nil {
		return "", err
	}
	return res.Stdout, nil
}

// Versions lists the arm64 builds available to fetch for a BSD
// ("freebsd"/"netbsd") as the non-empty output lines.
func (SystemNamespace) Versions(osName string) ([]string, error) {
	res, err := RunChecked([]string{"versions", "--os", osName}, "bsdkrun versions", nil)
	if err != nil {
		return nil, err
	}
	var out []string
	for _, line := range strings.Split(res.Stdout, "\n") {
		if trimmed := strings.TrimSpace(line); trimmed != "" {
			out = append(out, trimmed)
		}
	}
	return out, nil
}

// GrowDisk grows a raw disk image (the guest expands its root FS on next
// boot).
func (SystemNamespace) GrowDisk(disk, size string) error {
	_, err := RunChecked([]string{"grow", "--disk", disk, "--size", size}, "bsdkrun grow", nil)
	return err
}

func orEmptyJSON(stdout string) string {
	if strings.TrimSpace(stdout) == "" {
		return "[]"
	}
	return stdout
}
