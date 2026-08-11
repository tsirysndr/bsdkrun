// Package dotnet builds C# projects.
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
