package bsdkrun

import (
	"encoding/json"
	"strings"
)

// ZfsNamespace groups ZFS volume operations; use the Zfs var.
//
// A ZFS volume is one zpool on a disk image, held by one machine at a time
// (Linux, FreeBSD or NetBSD). Its branches are datasets cloned copy-on-write
// inside the pool, so a branch costs nothing until it diverges.
type ZfsNamespace struct{}

// Zfs is the ZFS volume namespace.
var Zfs ZfsNamespace

// List lists ZFS volumes, with their branches and holder.
func (ZfsNamespace) List() ([]ZfsVolumeInfo, error) {
	res, err := RunChecked([]string{"zfs", "ls", "--json"}, "bsdkrun zfs ls", nil)
	if err != nil {
		return nil, err
	}
	var rows []ZfsVolumeInfo
	if err := json.Unmarshal([]byte(orEmptyJSON(res.Stdout)), &rows); err != nil {
		return nil, err
	}
	return rows, nil
}

// Branches lists the branches of one volume.
func (ZfsNamespace) Branches(volume string) ([]ZfsBranchInfo, error) {
	res, err := RunChecked([]string{"zfs", "ls", volume, "--json"}, "bsdkrun zfs ls", nil)
	if err != nil {
		return nil, err
	}
	var rows []ZfsBranchInfo
	if err := json.Unmarshal([]byte(orEmptyJSON(res.Stdout)), &rows); err != nil {
		return nil, err
	}
	return rows, nil
}

// Create makes a volume: a sparse disk image that becomes a zpool with a
// "main" branch the first time a guest imports it. size is e.g. "10G"; empty
// means the CLI default.
func (ZfsNamespace) Create(name, size string) (string, error) {
	args := []string{"zfs", "create", name}
	if size != "" {
		args = append(args, "--size", size)
	}
	res, err := RunChecked(args, "bsdkrun zfs create", nil)
	if err != nil {
		return "", err
	}
	return strings.TrimSpace(res.Stdout), nil
}

// Branch forks a branch copy-on-write (zfs snapshot + zfs clone) from the
// branch named by from ("main" when empty). It runs inside the machine holding
// the volume, or in a short-lived FreeBSD helper, so it can take as long as a
// BSD boot. Returns the new "VOL@BRANCH".
func (ZfsNamespace) Branch(volume, name, from string) (string, error) {
	args := []string{"zfs", "branch", volume, name}
	if from != "" {
		args = append(args, "--from", from)
	}
	res, err := RunChecked(args, "bsdkrun zfs branch", nil)
	if err != nil {
		return "", err
	}
	return strings.TrimSpace(res.Stdout), nil
}

// Attach attaches "VOL" or "VOL@BRANCH" to a machine, mounting the branch at
// mount ("/mnt/VOL", or "/mnt/VOL-BRANCH", when empty). A pool is imported by
// one machine at a time: a stopped machine picks it up on start, a running
// holder mounts the branch live. Returns the CLI's one-line report.
func (ZfsNamespace) Attach(target, machine, mount string) (string, error) {
	args := []string{"zfs", "attach", target, machine}
	if mount != "" {
		args = append(args, "--mount", mount)
	}
	res, err := RunChecked(args, "bsdkrun zfs attach", nil)
	if err != nil {
		return "", err
	}
	return strings.TrimSpace(res.Stdout), nil
}

// Detach detaches one branch ("VOL@BRANCH") or the whole volume ("VOL") from
// a machine.
func (ZfsNamespace) Detach(target, machine string) (string, error) {
	res, err := RunChecked([]string{"zfs", "detach", target, machine}, "bsdkrun zfs detach", nil)
	if err != nil {
		return "", err
	}
	return strings.TrimSpace(res.Stdout), nil
}

// Destroy destroys branches ("VOL@BRANCH") or whole volumes ("VOL").
func (ZfsNamespace) Destroy(targets ...string) error {
	return destroyZfs(false, targets)
}

// ForceDestroy destroys volumes even when a stopped machine has them attached.
func (ZfsNamespace) ForceDestroy(targets ...string) error {
	return destroyZfs(true, targets)
}

func destroyZfs(force bool, targets []string) error {
	args := []string{"zfs", "destroy"}
	if force {
		args = append(args, "--force")
	}
	args = append(args, targets...)
	_, err := RunChecked(args, "bsdkrun zfs destroy", nil)
	return err
}
