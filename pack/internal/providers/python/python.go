// Package python builds Python projects, with the interpreter installed by
// mise rather than taken from a language base image.
//
// Every other interpreted provider here starts from the language's official
// image (ruby:3.2-bookworm, php:8.2-cli-bookworm). Python starts from plain
// Debian and installs the interpreter with mise, for two reasons:
//
//   - mise's Python builds are relocatable. python-build-standalone links
//     its dependencies — OpenSSL, SQLite, zlib — statically into the
//     interpreter, so what lands in the rootfs is the tree mise installed
//     plus libc, rather than a scatter of system libraries that a
//     distribution's python happens to have been compiled against.
//   - A project that pins python in .tool-versions or mise.toml gets that
//     exact version, not the nearest tag that exists on Docker Hub.
package python

import (
	"fmt"
	"os"
	"path/filepath"

	"github.com/tsirysndr/bsdkrun/pack/internal/plan"
	"github.com/tsirysndr/bsdkrun/pack/internal/providers/entry"
	"github.com/tsirysndr/bsdkrun/pack/internal/versions"
)

const defaultVersion = "3.12"

// prefix is where the interpreter lands in the guest. Python locates its
// own standard library relative to the executable — bin/python3 implies
// lib/python3.x/ beside it — so the tree has to be copied whole, to one
// prefix, rather than split across /usr/bin and /usr/lib.
const prefix = "/opt/python"

type Provider struct{}

func New() *Provider { return &Provider{} }

func (p *Provider) Name() string { return "python" }

// markers are the dependency manifests, in no particular order — any one of
// them means this is a Python project.
var markers = []string{
	"requirements.txt",
	"pyproject.toml",
	"uv.lock",
	"Pipfile",
	"Pipfile.lock",
	"setup.py",
	".python-version",
}

var scripts = []string{"main.py", "app.py", "server.py", "wsgi.py", "manage.py"}

func (p *Provider) Detect(dir string) (bool, error) {
	for _, marker := range markers {
		_, err := os.Stat(filepath.Join(dir, marker))
		if err == nil {
			return true, nil
		}
		if !os.IsNotExist(err) {
			return false, err
		}
	}
	// A single-file service need carry no manifest at all, so a
	// conventional entrypoint counts as a marker in its own right.
	_, ok := entry.Find(dir, scripts)
	return ok, nil
}

func (p *Provider) StartCommandHelp() string {
	return `Python runs main.py (or app.py / server.py). A Procfile "web:" line overrides it.`
}

func (p *Provider) Plan(dir string, _ plan.Arch) (*plan.Plan, error) {
	main := entry.FindOr(dir, scripts, "main.py")

	version := defaultVersion
	if v, ok := versions.Read(dir).Version("python"); ok {
		version = v
	}

	return &plan.Plan{
		Name:       "python",
		Provider:   p.Name(),
		BuildImage: "debian:bookworm-slim",
		Env: map[string]string{
			"PATH": "/root/.local/bin:/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin",
			// Without this mise builds CPython from source, which turns a
			// one-minute build into a twenty-minute one. It wants a full
			// toolchain and every -dev header too, none of which are in a
			// slim base image, so the source path does not merely take
			// longer here — it fails.
			"MISE_PYTHON_COMPILE": "0",
			"MISE_DATA_DIR":       "/opt/mise",
			"MISE_YES":            "1",
		},
		// uv installs packages an order of magnitude faster than pip, and
		// is the only thing here that can read a uv.lock. It ships as a
		// single static binary in its own image, so taking it from there
		// costs one cached layer and no install step.
		//
		// The tag is a moving one: uv publishes no stable major-version
		// tag the way composer:2 does, and pinning a patch release here
		// would mean this file needs editing every few weeks to keep
		// building.
		Tools: []plan.ToolCopy{
			{Image: "ghcr.io/astral-sh/uv:latest", Src: "/uv", Dst: "/usr/local/bin/uv"},
		},
		Kconfig: map[string]string{
			// The interpreter is not under any of the base LD_LIBRARY_PATH
			// prefixes, and PYTHONHOME spares it from having to deduce its
			// own prefix under an ELF loader where argv[0] is not what it
			// would be under Linux.
			"CONFIG_LIBPOSIX_ENVIRON_ENVP4": fmt.Sprintf(`"PYTHONHOME=%s"`, prefix),
			// Console output through a unikernel's serial port is worth
			// nothing if it sits in a buffer the guest never flushes: a
			// crash then takes the log with it.
			"CONFIG_LIBPOSIX_ENVIRON_ENVP5": `"PYTHONUNBUFFERED=1"`,
			// The rootfs is a read-only ramdisk, so a .pyc write can only
			// fail. Python tolerates that, but silently retries on every
			// import.
			"CONFIG_LIBPOSIX_ENVIRON_ENVP6": `"PYTHONDONTWRITEBYTECODE=1"`,
		},
		Script: fmt.Sprintf(`set -eu
%sapt-get update -qq
apt-get install -y -qq --no-install-recommends ca-certificates curl git xz-utils >/dev/null

curl -fsSL https://mise.run | sh
mise install "python@%[2]s"
PYROOT=$(mise where "python@%[2]s")
PY="$PYROOT/bin/python3"

# Dependencies. The manifests are checked most-specific first: a project
# using uv or pipenv often also carries a requirements.txt generated from
# its lock file, and it is the lock file that is authoritative.
#
# uv does the installing wherever it can, pointed at the interpreter mise
# installed so that packages land in its site-packages. A virtualenv would
# work too, but it would put a second prefix in the rootfs for the guest to
# find, and the point of installing into $PYROOT is that there is exactly
# one place Python has to look.
if [ -f uv.lock ]; then
    uv export --frozen --no-dev --no-emit-project --format requirements-txt \
        -o /tmp/requirements.txt
    uv pip install --python "$PY" --quiet -r /tmp/requirements.txt
elif [ -f Pipfile.lock ] || [ -f Pipfile ]; then
    # uv cannot read a Pipfile, so this is the one path that still needs
    # pipenv — used only to render the lock as requirements.
    uv pip install --python "$PY" --quiet pipenv
    "$PYROOT"/bin/pipenv requirements > /tmp/requirements.txt
    uv pip install --python "$PY" --quiet -r /tmp/requirements.txt
elif [ -f pyproject.toml ]; then
    uv pip install --python "$PY" --quiet .
elif [ -f requirements.txt ]; then
    uv pip install --python "$PY" --quiet -r requirements.txt
fi

# Parts of the standard library that cannot be reached from a unikernel: a
# test suite, two GUI toolkits, and a bootstrap copy of pip that has already
# served its purpose above. Together they are most of the interpreter's
# on-disk size, and the rootfs is a ramdisk the guest pays for in RAM.
rm -rf "$PYROOT"/lib/python*/test \
       "$PYROOT"/lib/python*/idlelib \
       "$PYROOT"/lib/python*/tkinter \
       "$PYROOT"/lib/python*/turtledemo \
       "$PYROOT"/lib/python*/ensurepip
find "$PYROOT" -type d -name __pycache__ -prune -exec rm -rf {} + 2>/dev/null || true

mkdir -p /out/rootfs%[3]s /out/rootfs/src /out/rootfs/tmp
cp -a "$PYROOT"/. /out/rootfs%[3]s/

# The interpreter's own libraries, plus those of every extension module:
# the stdlib ships compiled extensions (_ssl, _sqlite3, ...) and the first
# import of one fails in the guest if its libraries were never copied.
{ ldd "$PY"
  find "$PYROOT" -name '*.so' -exec ldd {} \; 2>/dev/null || true; } \
  | grep -oE '/[^ ()]+' \
  | sort -u \
  | while read -r lib; do
        mkdir -p "/out/rootfs$(dirname "$lib")"
        cp -L "$lib" "/out/rootfs$lib"
    done

cp -a . /out/rootfs/src/ 2>/dev/null || true
chmod 1777 /out/rootfs/tmp
`, plan.LddIntoRootfs, version, prefix),
		// -u belongs on the command line as well as in the environment:
		// PYTHONUNBUFFERED reaches the interpreter only if the guest's
		// environment survived, and this way a missing ENVP costs nothing.
		Cmd: []string{prefix + "/bin/python3", "-u", "/src/" + main},
	}, nil
}
