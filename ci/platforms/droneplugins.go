package platforms

// Real Drone plugins, without a Docker daemon. A Drone plugin is a
// container image speaking the simplest protocol of all: its settings
// arrive as PLUGIN_* environment variables and its entrypoint does the
// work. The runner already knows how to pull images — so at plan time the
// plugin image's rootfs is pulled host-side (`bsdkrun image pull --json`)
// and mounted read-only into the guest, and at run time the step builds a
// writable overlay over it, bind-mounts the workspace at /drone/src,
// enters via chroot with the image's own env plus the flattened settings,
// and runs the entrypoint. No shell is required inside the image — many
// plugin images are scratch plus one static binary — and such a plugin
// starts in /, exactly as it would under a runtime with no WORKDIR, with
// DRONE_WORKSPACE naming the workspace it should write to.
//
// Plugins that talk to a Docker daemon (plugins/docker building images)
// still fail inside, with their own error — the same honest boundary as
// container actions.

import (
	"fmt"
	"sort"
	"strings"

	"gopkg.in/yaml.v3"
)

// PulledImage is what the host-side pull reports.
type PulledImage struct {
	Rootfs     string   `json:"rootfs"`
	Entrypoint []string `json:"entrypoint"`
	Cmd        []string `json:"cmd"`
	Env        []string `json:"env"`
	Workdir    string   `json:"workdir"`
}

// PullImageFunc pulls an image host-side. Package-level and swappable so
// tests inject fixtures; the CLI wires the real implementation (an exec of
// the bsdkrun that launched it).
var PullImageFunc func(ref string) (PulledImage, error) = func(ref string) (PulledImage, error) {
	return PulledImage{}, fmt.Errorf("image pulling is not wired in this context")
}

// droneSettingsEnv flattens `settings:` into PLUGIN_* exactly as Drone
// does: keys uppercased, scalars stringified, lists comma-joined, maps as
// JSON.
func droneSettingsEnv(n yaml.Node) map[string]string {
	if n.IsZero() {
		return nil
	}
	var m map[string]interface{}
	if err := n.Decode(&m); err != nil {
		return nil
	}
	out := map[string]string{}
	for k, v := range m {
		out["PLUGIN_"+strings.ToUpper(strings.ReplaceAll(k, "-", "_"))] = droneValue(v)
	}
	return out
}

func droneValue(v interface{}) string {
	switch val := v.(type) {
	case nil:
		return ""
	case string:
		return val
	case bool:
		return fmt.Sprintf("%t", val)
	case int:
		return fmt.Sprintf("%d", val)
	case []interface{}:
		parts := make([]string, len(val))
		for i, item := range val {
			parts[i] = droneValue(item)
		}
		return strings.Join(parts, ",")
	case map[string]interface{}:
		// Drone encodes nested maps as JSON strings.
		var b strings.Builder
		b.WriteString("{")
		keys := make([]string, 0, len(val))
		for k := range val {
			keys = append(keys, k)
		}
		sort.Strings(keys)
		for i, k := range keys {
			if i > 0 {
				b.WriteString(",")
			}
			fmt.Fprintf(&b, "%q:%q", k, droneValue(val[k]))
		}
		b.WriteString("}")
		return b.String()
	default:
		return fmt.Sprintf("%v", val)
	}
}

// dronePluginMountDir is where a plugin image's rootfs lands in the guest.
func dronePluginMountDir(name string) string {
	return "/tangled/.drone/img/" + sanitizeBk(name)
}

// chrootExecScript builds the shell that runs one containerized step
// without a Docker daemon: a writable root over the read-only image rootfs
// mounted at imgDir, with the run's workspace bound at bindDst inside it.
// Exactly one of argv (exec'd directly) or script (written into the root
// and exec'd, so its shebang picks the interpreter) runs.
//
// chroot(1) chdirs to the new root, so the working directory has to be set
// on the other side of it — a probe showed a chrooted step starting in /
// no matter what the outer shell had chdir'd to. A script can just cd
// itself; an argv goes through the image's own /bin/sh when it has one,
// and otherwise runs from / (which is what a scratch plugin image gets —
// Drone plugins read DRONE_WORKSPACE for their working directory anyway).
func chrootExecScript(imgDir, bindDst, workdir string, argv []string, script string) string {
	var launch string
	if script != "" {
		body := script
		if !strings.HasPrefix(body, "#!") {
			body = "#!/bin/sh\nset -e\n" + body
		}
		// The cd belongs inside the script: chroot lands the process in /.
		if workdir != "" {
			lines := strings.SplitN(body, "\n", 2)
			body = lines[0] + "\ncd " + bkQuote(workdir) + "\n"
			if len(lines) > 1 {
				body += lines[1]
			}
		}
		launch = fmt.Sprintf(`cat > "$W/root/tmp/bsdkrun-step" <<'BSDKRUN_STEP_EOF'
%s
BSDKRUN_STEP_EOF
chmod +x "$W/root/tmp/bsdkrun-step"
chroot "$W/root" /tmp/bsdkrun-step`, body)
	} else {
		quoted := make([]string, len(argv))
		for i, a := range argv {
			quoted[i] = bkQuote(a)
		}
		joined := strings.Join(quoted, " ")
		launch = fmt.Sprintf(`if [ -x "$W/root/bin/sh" ]; then
  chroot "$W/root" /bin/sh -c 'cd %[1]s && exec "$@"' bsdkrun %[2]s
else
  # No shell in the image (scratch plus one binary): run from /, which is
  # what a container runtime would give a plugin with no WORKDIR either.
  chroot "$W/root" %[2]s
fi`, bkQuote(workdir), joined)
	}
	return fmt.Sprintf(`set -e
W=/tmp/bsdkrun-chroot-$$
mkdir -p "$W"
# The upperdir must live on a filesystem overlayfs accepts as upper. On a
# Linux host the guest's /tmp is virtiofs-backed, and overlay over a
# virtiofs upper mounts SILENTLY read-only (macOS guests have a tmpfs
# /tmp, which is why this only broke on x86_64/KVM). A dedicated tmpfs
# always qualifies.
mount -t tmpfs tmpfs "$W"
mkdir -p "$W/up" "$W/work" "$W/root"
# The share must have arrived, or the chroot below fails as a confusing
# ENOENT from exec rather than as the missing mount it is.
[ -n "$(ls -A %[1]s 2>/dev/null)" ] || {
  echo "image rootfs at %[1]s is empty in the guest — the read-only share did not arrive"
  exit 1
}
# overlayfs does not reliably stack on the filesystem an image rootfs arrives
# over. Two ways it has failed here, neither of them loudly: with a virtio-fs
# lower it can mount EMPTY instead of failing, and it can mount populated but
# reject writes with EOPNOTSUPP when a directory has to be copied up ("can't
# create ...: Not supported"). So the mount's exit code proves nothing — the
# test is whether the merged root has content AND accepts a file in a
# subdirectory, which is exactly what running a step needs.
overlay_ok=0
if mount -t overlay overlay -o lowerdir=%[1]s,upperdir="$W/up",workdir="$W/work" "$W/root" 2>/dev/null; then
  if [ -n "$(ls -A "$W/root" 2>/dev/null)" ] &&
     mkdir -p "$W/root/tmp" 2>/dev/null &&
     touch "$W/root/tmp/.rwcheck" 2>/dev/null; then
    rm -f "$W/root/tmp/.rwcheck"
    overlay_ok=1
  fi
fi
if [ "$overlay_ok" -ne 1 ]; then
  # Copying costs RAM equal to the image, and always works: the tmpfs is
  # ours, so there is no lower filesystem left to disagree with.
  echo "[bsdkrun] overlay is unusable over %[1]s — copying the rootfs into tmpfs instead"
  umount "$W/root" 2>/dev/null || true
  rm -rf "$W/root" && mkdir -p "$W/root"
  cp -a %[1]s/. "$W/root/"
fi
mkdir -p "$W/root%[2]s" "$W/root/proc" "$W/root/dev" "$W/root/etc" "$W/root/tmp" "$W/root/root"
mount --bind /tangled/workspace "$W/root%[2]s"
mount --bind /proc "$W/root/proc" 2>/dev/null || mount -t proc proc "$W/root/proc" || true
mount --bind /dev "$W/root/dev" 2>/dev/null || true
# DNS: the chroot gets its own /etc, so without this a plugin resolves
# against nothing and Go's resolver falls back to localhost — the failure
# reads "lookup <host> on [::1]:53: connection refused", which looks like a
# network outage and is not one. cat rather than cp, because the guest's
# resolv.conf may be a symlink; and if what lands has no nameserver, derive
# one from the default route rather than leaving the plugin blind.
cat /etc/resolv.conf > "$W/root/etc/resolv.conf" 2>/dev/null || true
if ! grep -q '^nameserver' "$W/root/etc/resolv.conf" 2>/dev/null; then
  __gw=$(ip route 2>/dev/null | awk '/^default/ {print $3; exit}')
  [ -n "$__gw" ] || __gw=192.168.127.1
  echo "[bsdkrun] the guest has no usable resolv.conf — pointing the plugin at $__gw"
  printf 'nameserver %s\n' "$__gw" > "$W/root/etc/resolv.conf"
fi
%[3]s`, bkQuote(imgDir), bindDst, launch)
}

// dronePluginStep builds the chroot-execution step for one plugin, and the
// read-only mount the VM needs for it. flavor is "drone" or "woodpecker":
// the PLUGIN_* protocol is identical, but woodpecker-native plugins read
// CI_* variables where drone plugins read DRONE_* — both are set for
// woodpecker, because most woodpecker plugins began life drone-compatible.
func dronePluginStep(stepName, image string, settings yaml.Node, repo Repo, flavor string) (Step, string, error) {
	pulled, err := PullImageFunc(image)
	if err != nil {
		return Step{}, "", fmt.Errorf("pulling plugin image %s: %w", image, err)
	}
	argv := append(append([]string{}, pulled.Entrypoint...), pulled.Cmd...)
	if len(argv) == 0 {
		return Step{}, "", fmt.Errorf("plugin image %s declares no entrypoint", image)
	}

	env := map[string]string{}
	for _, kv := range pulled.Env {
		if k, v, ok := strings.Cut(kv, "="); ok {
			env[k] = v
		}
	}
	for k, v := range droneSettingsEnv(settings) {
		env[k] = v
	}
	env["DRONE"] = "true"
	env["DRONE_COMMIT_SHA"] = repo.Sha
	env["DRONE_WORKSPACE"] = "/drone/src"
	env["CI"] = "true"
	env["HOME"] = "/root"
	if flavor == "woodpecker" {
		env["CI"] = "woodpecker"
		env["CI_COMMIT_SHA"] = repo.Sha
		env["CI_WORKSPACE"] = "/drone/src"
	}

	guestImg := dronePluginMountDir(sanitizeBk(image))
	cmd := chrootExecScript(guestImg, "/drone/src", "/drone/src", argv, "")

	return Step{
		Name:    stepName + " [plugin: " + image + "]",
		Command: cmd,
		Env:     env,
	}, pulled.Rootfs + ":" + guestImg + ":ro", nil
}
