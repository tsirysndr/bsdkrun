// Package beam holds what the Elixir and Gleam providers share: carving a
// runnable Erlang runtime out of a full OTP install.
//
// Ported from examples/unikraft-elixir and examples/unikraft-gleam, whose
// Dockerfiles arrived at the same script. A unikernel image is resident
// twice at boot (embedded in the kernel *and* unpacked into ramfs), so
// shipping all of /usr/local/lib/erlang is not an option; this copies the
// three ERTS programs that are actually exec'd and then walks the
// `{applications, [...]}` graph from the release's own .app files to pull in
// exactly the OTP libraries it transitively needs.
package beam

import (
	"fmt"

	"github.com/tsirysndr/bsdkrun/pack/internal/plan"
)

// GleamApps selects OTP applications by walking the {applications,[...]}
// graph from the shipment's own .app files. `gleam export erlang-shipment`
// lays them out as srv/<app>/ebin/<app>.app, so the glob finds them.
const GleamApps = `deps_of() {
    tr -d ' \n' < "$1" \
      | sed -n 's/.*{applications,\[\([^]]*\)\].*/\1/p' \
      | tr ',' '\n'
    echo
}
queue=$(for f in /out/rootfs/srv/*/ebin/*.app; do deps_of "$f"; done | sort -u)
seen=' '
while [ -n "$queue" ]; do
    next=''
    for app in $queue; do
        case "$seen" in *" $app "*) continue ;; esac
        seen="$seen$app "
        d=$(echo "$ERL/lib/$app"-*)
        [ -d "$d" ] || continue
        cp -a "$d" /out/rootfs/erl/lib/
        next="$next $(deps_of "$d/ebin/$app.app")"
    done
    queue=$(echo "$next" | tr ' ' '\n' | sort -u)
done
`

// ElixirApps selects OTP applications from the release's start.script.
//
// Which applications to carry is not a guess for a mix release: start.script
// records the $ROOT-relative code path of every application the boot script
// loads. The graph walk GleamApps uses would find nothing here, because a
// mix release puts its own applications under $RELEASE_LIB (srv/lib/...)
// rather than srv/<app>/ebin — and the result is a VM that dies at boot with
// load_failed on every kernel module.
const ElixirApps = `grep -o '"\$ROOT/lib/[^"]*/ebin"' /out/rootfs/srv/releases/*/start.script \
  | sed 's|.*/lib/\(.*\)/ebin"|\1|' \
  | sort -u \
  | while read -r app; do
        cp -a "$ERL/lib/$app" /out/rootfs/erl/lib/
    done
`

// ExtractERTS returns the shell that builds /out/rootfs/erl from the image's
// OTP install, using the given application-selection step. The release is
// expected to already be at /out/rootfs/srv.
func ExtractERTS(selectApps string) string {
	return fmt.Sprintf(extractERTS, selectApps)
}

// extractERTS is that script with a %s where the selection step goes.
const extractERTS = `
ERL=/usr/local/lib/erlang
ERTS=$(basename "$ERL"/erts-*)
mkdir -p "/out/rootfs/erl/$ERTS/bin" /out/rootfs/erl/lib /out/rootfs/root /out/rootfs/tmp

# Only these three are ever exec'd: the emulator itself, the helper it
# spawns for ports, and the DNS resolver.
for prog in beam.smp erl_child_setup inet_gethost; do
    cp -a "$ERL/$ERTS/bin/$prog" "/out/rootfs/erl/$ERTS/bin/$prog"
done
ln -s "$ERTS" /out/rootfs/erl/erts

mkdir -p /out/rootfs/erl/bin
cp -a "$ERL"/bin/*.boot /out/rootfs/erl/bin/

%s
echo "OTP applications carried:"; ls /out/rootfs/erl/lib

# Drop what a running system never reads, before resolving libraries — the
# image is resident twice at boot, so this is size that matters.
find /out/rootfs/erl -type d \
    \( -name src -o -name doc -o -name examples -o -name man -o -name include \) \
    -prune -exec rm -rf {} +

# Every executable and .so under the whole tree, not just beam.smp: OTP's
# NIFs are what pull in the real dependencies. crypto's priv/lib/crypto.so
# is why libcrypto.so.3 ends up in the image, and without it the VM dies at
# boot with "Unable to load crypto library".
find /out/rootfs/erl -type f \( -perm -u+x -o -name '*.so' \) -print \
  | while read -r f; do ldd "$f" 2>/dev/null || true; done \
  | grep -oE '/[^ ()]+' \
  | grep -v '^/out/rootfs' \
  | sort -u \
  | while read -r lib; do
        [ -f "$lib" ] || continue
        mkdir -p "/out/rootfs$(dirname "$lib")"
        cp -L "$lib" "/out/rootfs$lib"
    done
chmod 1777 /out/rootfs/tmp
`

// Env is the environment erlexec would normally set up. beam.smp is started
// directly here — there is no shell in the guest to run the release's start
// script — so these have to be baked into the kernel config instead.
func Env() map[string]string {
	return map[string]string{
		"CONFIG_LIBPOSIX_ENVIRON_ENVP4": `"ROOTDIR=/erl"`,
		"CONFIG_LIBPOSIX_ENVIRON_ENVP5": `"BINDIR=/erl/erts/bin"`,
		"CONFIG_LIBPOSIX_ENVIRON_ENVP6": `"EMU=beam"`,
		"CONFIG_LIBPOSIX_ENVIRON_ENVP7": `"PROGNAME=erl"`,
	}
}

// Argv is the argument vector erlexec would have built, up to the point
// where the caller appends what to actually run.
//
// The `--` separators are load-bearing and so is `-fnu`: the image ships no
// locale data, so setlocale() fails and the VM would otherwise settle on
// latin1 and warn that the runtime may malfunction.
func Argv() []string {
	return []string{
		"/erl/erts/bin/beam.smp", "-fnu", "--",
		"-root", "/erl", "-bindir", "/erl/erts/bin", "-progname", "erl", "--",
		"-home", "/root", "--",
	}
}

// Plan fills in the parts of a BEAM plan that Elixir and Gleam share.
func Plan(p *plan.Plan) *plan.Plan {
	if p.Kconfig == nil {
		p.Kconfig = map[string]string{}
	}
	for k, v := range Env() {
		p.Kconfig[k] = v
	}
	return p
}
