package platforms

// Real Drone plugins, without a Docker daemon. A Drone plugin is a
// container image speaking the simplest protocol of all: its settings
// arrive as PLUGIN_* environment variables and its entrypoint does the
// work. The runner already knows how to pull images — so at plan time the
// plugin image's rootfs is pulled host-side (`bsdkrun image pull --json`)
// and mounted read-only into the guest, and at run time the step builds a
// writable overlay over it, bind-mounts the workspace at /drone/src,
// enters via chroot with the image's own env plus the flattened settings,
// and runs the entrypoint. The cwd-survives-chroot trick supplies the
// working directory without needing a shell inside the image — many plugin
// images are scratch plus one static binary.
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
	quotedArgv := make([]string, len(argv))
	for i, a := range argv {
		quotedArgv[i] = bkQuote(a)
	}

	cmd := fmt.Sprintf(`set -e
# Writable overlay over the read-only plugin rootfs, workspace bound at
# /drone/src, and the cwd-survives-chroot trick for the working directory:
# chdir to the target through the overlay path first, then chroot — the
# kernel keeps the cwd inode, so the entrypoint starts in /drone/src with
# no shell needed inside the image.
W=/tmp/drone-plugin-$$
mkdir -p "$W/up" "$W/work" "$W/root"
mount -t overlay overlay -o lowerdir=%[1]s,upperdir="$W/up",workdir="$W/work" "$W/root"
mkdir -p "$W/root/drone/src" "$W/root/proc" "$W/root/dev" "$W/root/etc" "$W/root/tmp" "$W/root/root"
mount --bind /tangled/workspace "$W/root/drone/src"
mount --bind /proc "$W/root/proc" 2>/dev/null || mount -t proc proc "$W/root/proc" || true
mount --bind /dev "$W/root/dev" 2>/dev/null || true
cp /etc/resolv.conf "$W/root/etc/resolv.conf" 2>/dev/null || true
cd "$W/root/drone/src"
chroot "$W/root" %[2]s`,
		bkQuote(guestImg), strings.Join(quotedArgv, " "))

	return Step{
		Name:    stepName + " [plugin: " + image + "]",
		Command: cmd,
		Env:     env,
	}, pulled.Rootfs + ":" + guestImg + ":ro", nil
}
