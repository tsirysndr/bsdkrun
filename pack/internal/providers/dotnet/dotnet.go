// Package dotnet builds C# projects.
//
// KNOWN BROKEN: the build works and the guest boots, but CoreCLR fails to
// start with E_OUTOFMEMORY (0x8007000E) before any managed code runs.
//
// The cause is not memory. A --strace trace (which needs the tracer patch
// in library/unikraft-base/patches/apply.sh, or it dies formatting its own
// output) ends like this:
//
//	openat("/proc/self/maps", O_RDONLY|O_CLOEXEC) = No such file (-2)
//	openat("/proc/self/maps", O_RDONLY|O_CLOEXEC) = No such file (-2)
//	futex(NULL, FUTEX_WAKE|FUTEX_PRIVATE_FLAG, 0x7fffffff) = OK
//	Failed to create CoreCLR, HRESULT: 0x8007000E
//
// CoreCLR reads /proc/self/maps to place its executable heap. Unikraft has
// no procfs, so the read fails and the runtime reports it as being out of
// memory — which is why every memory-shaped theory about this was wrong.
//
// Ruled out along the way, each with evidence rather than argument:
//   - Guest RAM: 3 GiB fails identically to 1 GiB.
//   - The regions GC's huge reservation: a 1 GiB PROT_NONE mmap succeeds,
//     and no mmap in the whole trace fails.
//   - Thread creation: clone() succeeds; the trace continues into the new
//     thread (gettid returns pid:3).
//   - getsid() returning ESRCH: a real bug, since fixed, and unrelated.
//
// A fix means giving the guest a /proc/self/maps that describes its own
// address space. A static file in the rootfs is the cheap experiment;
// whether CoreCLR accepts one that does not match reality is untested.
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

mkdir -p /out/rootfs/usr/src/app /out/rootfs/tmp
cp -a /tmp/publish/. /out/rootfs/usr/src/app/

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
