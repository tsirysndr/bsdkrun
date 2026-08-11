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

import "github.com/tsirysndr/bsdkrun/pack/internal/plan"

// ExtractERTS is the shell that builds /out/rootfs/erl from the image's OTP
// install. The release is expected to already be at /out/rootfs/srv.
const ExtractERTS = `
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

deps_of() {
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

ldd_into_rootfs "/out/rootfs/erl/$ERTS/bin/beam.smp"
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
