// Package dotnet builds C# projects.
//
// CoreCLR failed to start here for a long time, with an E_OUTOFMEMORY
// (0x8007000E) that had nothing to do with memory. The cause was found by
// differential tracing — the same self-contained binary strace'd under
// Docker, aligned against a guest --strace (which itself needs the tracer
// patch in the Unikraft patches, or it dies formatting its own output) —
// and it took three coordinated fixes:
//
//  1. DOTNET_EnableWriteXorExecute=0 (ENVP10). .NET 8 turns W^X on by
//     default, and its executable allocator dual-maps JIT pages through
//     memfd_create — which Unikraft does not implement. The trace shows
//     memfd_create = -ENOSYS shortly before the failure.
//  2. A /proc/self/maps with a real [stack] line (written into the rootfs
//     below). glibc's pthread_getattr_np computes the main thread's stack
//     bounds by finding the maps line containing __libc_stack_end; CoreCLR
//     asks for those bounds during PAL startup and fails without them. The
//     good trace shows the tell: the maps read paired with
//     prlimit64(RLIMIT_STACK).
//  3. The RLIMIT_STACK patch in the Unikraft patches. Unikraft reported
//     the kernel thread stack size (64 KiB) while the app runs on the
//     elfloader's 512 KiB stack; glibc clamps the computed bounds by that
//     rlimit, producing a range that excluded the very stack pointer the
//     thread was running on.
//
// (1) was applied on direct evidence but not proven necessary in
// isolation; (2)+(3) are what turned the guest from failing to serving —
// with W^X off but the stack bounds still wrong, it failed identically.
//
// Ruled out on the way, so nobody re-tests them: guest RAM (3 GiB failed
// like 1 GiB), the regions GC's reservation (a 1 GiB PROT_NONE mmap
// succeeds), thread creation (clone succeeds), CPU count, getsid, and
// procfs stand-ins for stat/meminfo/mountinfo/cpu (supplied, read, no
// difference — only the maps [stack] line mattered).
package dotnet

import (
	"encoding/json"
	"fmt"
	"os"
	"path/filepath"
	"strings"

	"github.com/tsirysndr/bsdkrun/pack/internal/plan"
)

const defaultSDK = "8.0"

// SDKVersionEnv overrides the SDK version, taking precedence over
// global.json.
const SDKVersionEnv = "CSHARP_SDK_VERSION"

type Provider struct{}

func New() *Provider { return &Provider{} }

func (p *Provider) Name() string { return "dotnet" }

func (p *Provider) Detect(dir string) (bool, error) {
	matches, err := filepath.Glob(filepath.Join(dir, "*.csproj"))
	if err != nil {
		return false, err
	}
	return len(matches) > 0, nil
}

func (p *Provider) StartCommandHelp() string {
	return fmt.Sprintf(`C# publishes self-contained. %s or global.json picks the SDK.`, SDKVersionEnv)
}

func (p *Provider) Plan(dir string, arch plan.Arch) (*plan.Plan, error) {
	sdk := SDKVersion(dir)
	name := projectName(dir)

	return &plan.Plan{
		Name:       "dotnet",
		Provider:   p.Name(),
		BuildImage: "mcr.microsoft.com/dotnet/sdk:" + sdk,
		Env: map[string]string{
			"DOTNET_CLI_TELEMETRY_OPTOUT":       "1",
			"DOTNET_NOLOGO":                     "1",
			"DOTNET_SKIP_FIRST_TIME_EXPERIENCE": "1",
		},
		Kconfig: map[string]string{
			// The runtime's own libraries sit beside the app rather than in
			// any system directory, and coreclr dlopen()s them by bare name.
			"CONFIG_LIBPOSIX_ENVIRON_ENVP4": `"LD_LIBRARY_PATH=/usr/src/app:/usr/local/lib:/usr/lib:/lib"`,
			// A single-CPU guest with no cgroup to read. Left to itself the
			// runtime sizes its thread pool and GC heaps from a CPU count it
			// cannot determine.
			"CONFIG_LIBPOSIX_ENVIRON_ENVP5": `"DOTNET_gcServer=0"`,
			"CONFIG_LIBPOSIX_ENVIRON_ENVP6": `"DOTNET_TieredCompilation=0"`,
			"CONFIG_LIBPOSIX_ENVIRON_ENVP7": `"DOTNET_EnableDiagnostics=0"`,
			// The default GC reserves its heap as one enormous virtual
			// range (regions, 256 GiB on 64-bit). Unikraft cannot serve a
			// reservation of that size, and CoreCLR reports the failure as
			// E_OUTOFMEMORY during startup — before a line of managed code
			// runs, and regardless of how much RAM the guest actually has.
			//
			// libclrgc.so is the segments-based collector .NET ships for
			// exactly this: constrained environments where the regions
			// reservation cannot be satisfied. It is in every self-contained
			// publish already.
			"CONFIG_LIBPOSIX_ENVIRON_ENVP8": `"DOTNET_GCName=libclrgc.so"`,
			// A hard limit keeps the segments GC from sizing itself against
			// a machine it cannot see.
			"CONFIG_LIBPOSIX_ENVIRON_ENVP9": `"DOTNET_GCHeapHardLimit=0x10000000"`,
			// THE fix, found by tracing: .NET 8 turns W^X on by default,
			// and its executable-memory allocator builds a dual mapping
			// (one RW view, one RX view of the same pages) out of
			// memfd_create — which Unikraft does not implement. The trace
			// shows memfd_create = -ENOSYS, sixty-six lines before
			// CoreCLR reports it as E_OUTOFMEMORY, an error that named
			// neither the syscall nor the feature. The maps reads just
			// before the failure were this same allocator hunting for
			// free address space.
			//
			// With W^X off the JIT maps its code RWX in one view, which
			// is how every other JIT here (JVM, BEAM, V8) already runs
			// on Unikraft — a single address space with no processes has
			// no isolation for W^X to preserve.
			"CONFIG_LIBPOSIX_ENVIRON_ENVP10": `"DOTNET_EnableWriteXorExecute=0"`,
		},
		// --self-contained, so the runtime ships with the app: there is no
		// package manager in the guest to install one, and a
		// framework-dependent publish would look fine here and fail to start
		// there.
		//
		// Trimming is deliberately off. It resolves what to keep by static
		// analysis, and anything reached by reflection — which is most of
		// what a web framework does at startup — is invisible to it.
		Script: fmt.Sprintf(`set -eu
%sdotnet publish -c Release -r linux-%s --self-contained true \
    -p:PublishTrimmed=false -p:PublishSingleFile=false \
    -o /tmp/publish

mkdir -p /out/rootfs/usr/src/app /out/rootfs/tmp /out/rootfs/proc/self
cp -a /tmp/publish/. /out/rootfs/usr/src/app/

# What glibc's pthread_getattr_np() needs to compute the main thread's
# stack bounds: a /proc/self/maps whose [stack] line contains
# __libc_stack_end. CoreCLR asks for those bounds during PAL startup, and
# without this file the call fails -- reported as E_OUTOFMEMORY, an error
# that names neither the file nor the reason.
#
# The [stack] range brackets where app-elfloader actually puts the stack
# (observed at 0x10003xxxxx in a --strace trace; the loader is
# deterministic, there is no ASLR). glibc takes the line's top as the
# stack base and sizes it by min(range, RLIMIT_STACK) -- which is why the
# RLIMIT_STACK patch in the Unikraft patches is the other half of this:
# the unpatched 64 KiB rlimit clamps the bounds into a range that excludes
# the very stack pointer the thread is running on.
#
# The libcoreclr line is the anchor the executable-memory allocator looks
# for when placing its heap; it is secondary but costs one line.
cat > /out/rootfs/proc/self/maps <<'MAPS'
1000000000-1000100000 r-xp 00000000 00:00 0                              /usr/src/app/libcoreclr.so
1000200000-1000400000 rw-p 00000000 00:00 0                              [stack]
MAPS


if [ ! -f /out/rootfs/usr/src/app/%s ]; then
    echo "published output has no %s apphost; is <OutputType>Exe</OutputType> set?" >&2
    ls -la /out/rootfs/usr/src/app >&2
    exit 1
fi
chmod +x /out/rootfs/usr/src/app/%s

# The apphost and every native library the runtime carries.
ldd_into_rootfs /out/rootfs/usr/src/app/%s
for so in /out/rootfs/usr/src/app/*.so; do
    [ -f "$so" ] && ldd_into_rootfs "$so"
done
chmod 1777 /out/rootfs/tmp
`, plan.LddIntoRootfs, rid(arch), name, name, name, name),
		Cmd: []string{"/usr/src/app/" + name},
	}, nil
}

// SDKVersion resolves the SDK: the environment first, then global.json,
// then a default.
func SDKVersion(dir string) string {
	if v := os.Getenv(SDKVersionEnv); v != "" {
		return v
	}
	if v := globalJSONVersion(filepath.Join(dir, "global.json")); v != "" {
		return v
	}
	return defaultSDK
}

// globalJSONVersion reads sdk.version out of a global.json and reduces it to
// the image tag's major.minor — global.json pins a feature band ("8.0.404"),
// which is not a tag that exists on the SDK image.
func globalJSONVersion(path string) string {
	body, err := os.ReadFile(path)
	if err != nil {
		return ""
	}
	var doc struct {
		SDK struct {
			Version string `json:"version"`
		} `json:"sdk"`
	}
	if err := json.Unmarshal(body, &doc); err != nil {
		return ""
	}
	parts := strings.Split(doc.SDK.Version, ".")
	if len(parts) < 2 {
		return ""
	}
	return parts[0] + "." + parts[1]
}

// rid is the .NET runtime identifier for the target.
func rid(arch plan.Arch) string {
	if arch == plan.ArchArm64 {
		return "arm64"
	}
	return "x64"
}

// projectName is the apphost's name, which .NET takes from the project file
// unless the project overrides AssemblyName.
func projectName(dir string) string {
	matches, _ := filepath.Glob(filepath.Join(dir, "*.csproj"))
	if len(matches) == 0 {
		return filepath.Base(dir)
	}
	if name := assemblyName(matches[0]); name != "" {
		return name
	}
	return strings.TrimSuffix(filepath.Base(matches[0]), ".csproj")
}

// assemblyName reads an <AssemblyName> override out of a .csproj.
func assemblyName(path string) string {
	body, err := os.ReadFile(path)
	if err != nil {
		return ""
	}
	_, after, ok := strings.Cut(string(body), "<AssemblyName>")
	if !ok {
		return ""
	}
	name, _, ok := strings.Cut(after, "</AssemblyName>")
	if !ok {
		return ""
	}
	return strings.TrimSpace(name)
}
