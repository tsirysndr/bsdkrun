package platforms

// Dagger: a CI config that is a program.
//
// A dagger module is code — `dagger.json` naming an SDK, with the functions in
// `.dagger/` — and running it means running the dagger CLI, which needs a
// container runtime to provision its engine into. So this platform does not
// translate anything: it builds the environment the CLI needs (a Docker daemon
// and the CLI itself) and calls the module.
//
// The environment is shared with `engine: dagger` in a tangled workflow, which
// is the same requirement arrived at from the other direction: there the user
// writes the steps and only needs dagger to be present. Both go through the
// helpers here so the two can never drift.
//
// Detection is deliberately last in the registry: a repository with a dagger
// module usually also has a CI config that *calls* dagger, and running that
// config is the more faithful reproduction of what its CI does.

import (
	"fmt"
	"os"
	"path/filepath"
)

// DaggerImage is the VM image a dagger run boots. docker:dind by default —
// the daemon is the hard requirement, and the CLI installs in seconds — with
// an override for the prebuilt flavor image (dind + CLI already in it), which
// turns those seconds into nothing.
func DaggerImage() string {
	if img := os.Getenv("BSDKRUN_CI_DAGGER_IMAGE"); img != "" {
		return img
	}
	return "docker:dind"
}

// DaggerMinMemMiB is the memory floor for a dagger run. The engine is a
// BuildKit daemon in a container inside the VM, and it is not a small one: at
// the default 2 GiB it thrashes or is killed, and the failure looks like a
// hung build rather than an out-of-memory.
const DaggerMinMemMiB = 4096

// DaggerEnv is what every dagger step needs in its environment.
func DaggerEnv() map[string]string {
	return map[string]string{
		// dind serves TLS on 2376 unless this is empty; the CLI talks to the
		// local socket, and a half-configured TLS daemon is a confusing way to
		// fail.
		"DOCKER_TLS_CERTDIR": "",
		"DOCKER_HOST":        "unix:///var/run/docker.sock",
		// The engine writes progress as a TTY animation otherwise, which in a
		// captured log is thousands of cursor-movement escapes.
		"NO_COLOR":                 "1",
		"DAGGER_NO_NAG":            "1",
		"_EXPERIMENTAL_DAGGER_TUI": "0",
	}
}

// DaggerSetupSteps bring the machine to the state the CLI assumes: a running
// dockerd, and dagger on PATH.
func DaggerSetupSteps() []Step {
	return []Step{
		{
			Name: "Start Docker",
			Command: `set -e
# dind's own entrypoint does the storage-driver detection and cgroup setup;
# running dockerd directly gets that wrong in ways that only show up later,
# under load, as a build that cannot mount anything.
if ! command -v dockerd-entrypoint.sh >/dev/null 2>&1; then
  echo "this image has no Docker daemon — a dagger run needs one (image: ` + "`docker:dind`" + ` or the bsdkrun dagger flavor)"
  exit 1
fi
# dockerd's sockets live under /var/run/docker, and /var/run is a symlink to
# /run — which is on the guest's virtio-fs rootfs. Binding a unix socket there
# is not something a passthrough filesystem reliably allows: on a Linux host
# dockerd dies with
#   listen unix /var/run/docker/libnetwork/<id>.sock: bind: permission denied
# which reads like a privilege problem and is a filesystem one. A tmpfs is
# kernel-backed and takes sockets everywhere, so put the runtime directory on
# one before the daemon wants it.
if ! mountpoint -q /run 2>/dev/null; then
  mount -t tmpfs -o mode=0755 tmpfs /run 2>/dev/null || true
fi
mkdir -p /run/docker /var/run/docker 2>/dev/null || true
echo "docker runtime dir: $(stat -f -c %T /var/run 2>/dev/null || echo unknown)"
if docker info >/dev/null 2>&1; then
  echo "docker is already running"
else
  nohup dockerd-entrypoint.sh dockerd >/var/log/dockerd.log 2>&1 &
  # Waiting on the socket is not enough: dind untars its storage first, and a
  # daemon that is listening but not ready refuses the engine container.
  for i in $(seq 1 60); do
    docker info >/dev/null 2>&1 && break
    sleep 1
  done
fi
docker info >/dev/null 2>&1 || {
  echo "dockerd did not come up; last lines of its log:"
  tail -20 /var/log/dockerd.log 2>/dev/null
  exit 1
}
docker version --format '{{.Server.Version}}' 2>/dev/null | sed 's/^/dockerd /'`,
		},
		{
			Name: "Install dagger",
			Command: `set -e
if command -v dagger >/dev/null 2>&1; then
  dagger version
  exit 0
fi
# The flavor image ships the CLI; a plain docker:dind does not, and installing
# it here is what keeps this working on any dind image.
command -v curl >/dev/null 2>&1 || apk add --no-cache curl >/dev/null 2>&1 || true
# DAGGER_VERSION pins the CLI; unset installs the latest stable. A project on
# the 1.0 line (dagger.toml / dagger-module.toml) needs a CLI from that line,
# and only the project knows which.
curl -fsSL https://dl.dagger.io/dagger/install.sh | BIN_DIR=/usr/local/bin sh
dagger version`,
			Env: daggerVersionEnv(),
		},
		{
			Name: "Pull the dagger engine",
			Command: `set -e
# The engine is a container the CLI starts, and it is a large image over a
# user-mode network: a single stalled blob makes ` + "`dagger call`" + ` fail with
# "failed to pull image", which reads like the image does not exist. Pulling it
# here, with retries, turns a flaky download into a slow one — and docker
# resumes what it already fetched, so a retry is not a restart.
VER=$(dagger version 2>/dev/null | awk '{print $2}')
[ -n "$VER" ] || { echo "dagger did not report a version"; exit 1; }
IMG="registry.dagger.io/engine:$VER"
if docker image inspect "$IMG" >/dev/null 2>&1; then
  echo "$IMG already present"
  exit 0
fi
for attempt in 1 2 3; do
  echo "pulling $IMG (attempt $attempt)"
  if docker pull "$IMG"; then exit 0; fi
  sleep 5
done
echo "could not pull $IMG after 3 attempts"
exit 1`,
		},
	}
}

// daggerVersionEnv pins the CLI when asked. The install script reads
// DAGGER_VERSION itself, so this is a pass-through rather than a translation.
func daggerVersionEnv() map[string]string {
	v := os.Getenv("BSDKRUN_CI_DAGGER_VERSION")
	if v == "" {
		return nil
	}
	return map[string]string{"DAGGER_VERSION": v}
}

// DaggerCallStep runs the module. The function is chosen in the guest rather
// than at plan time because only a provisioned engine can say what a module
// exposes: `dagger functions` needs the engine that `dagger call` needs.
func DaggerCallStep(fn string) Step {
	return Step{
		Name: "dagger call",
		Command: fmt.Sprintf(`set -e
FN=%q
if [ -z "$FN" ]; then
  # No function named, so take the first conventional one this module has.
  AVAILABLE=$(dagger functions 2>/dev/null | awk 'NR>1 {print $1}')
  for candidate in ci test build all; do
    if printf '%%s\n' "$AVAILABLE" | grep -qx "$candidate"; then
      FN="$candidate"
      break
    fi
  done
  if [ -z "$FN" ]; then
    echo "no ci, test, build or all function in this module. It exposes:"
    dagger functions
    echo
    echo "run one with: bsdkrun ci run --dagger-call <function>"
    exit 1
  fi
  echo "calling $FN (no function named; add --dagger-call to choose)"
fi
dagger call "$FN"`, fn),
	}
}

// daggerConfigFiles are the shapes a dagger project takes, across the version
// boundary that is currently open:
//
//   - `dagger.json` — the module config through 0.21.x, and still the
//     LegacyFilename the 1.0 line reads.
//   - `dagger-module.toml` — what 1.0 renamed it to (core/modules/config.go:
//     `const Filename = "dagger-module.toml"`).
//   - `dagger.toml` — 1.0's *workspace* file, a different thing again: it
//     lists `[modules.<name>] source = …` for a repository holding several.
//
// All three are detected, because a repository is on whichever the version its
// authors use writes, and a runner that only knew one would silently ignore
// the others.
var daggerConfigFiles = []string{"dagger.json", "dagger.toml", "dagger-module.toml"}

func detectDagger(root string) bool {
	for _, f := range daggerConfigFiles {
		if fileExists(filepath.Join(root, f)) {
			return true
		}
	}
	// A module can also live entirely under .dagger/ with its config beside
	// the source rather than at the root.
	if st, err := os.Stat(filepath.Join(root, ".dagger")); err == nil && st.IsDir() {
		return true
	}
	return false
}

// DaggerCall is the function name to invoke, set from `--dagger-call`. Empty
// means "pick a conventional one in the guest".
var DaggerCall string

func loadDagger(root string, repo Repo) ([]Job, error) {
	job := Job{
		Name:      "dagger",
		Image:     DaggerImage(),
		Env:       DaggerEnv(),
		MinMemMiB: DaggerMinMemMiB,
		Steps:     append(DaggerSetupSteps(), DaggerCallStep(DaggerCall)),
	}
	return []Job{job}, nil
}
