// Package clojure builds Clojure projects. Ported from
// examples/unikraft-clojure.
package clojure

import (
	"os"
	"path/filepath"

	"github.com/tsirysndr/bsdkrun/pack/internal/plan"
	"github.com/tsirysndr/bsdkrun/pack/internal/versions"
)

const defaultJDK = "21"

type Provider struct{}

func New() *Provider { return &Provider{} }

func (p *Provider) Name() string { return "clojure" }

func (p *Provider) Detect(dir string) (bool, error) {
	for _, marker := range []string{"deps.edn", "project.clj", "build.boot", "shadow-cljs.edn"} {
		_, err := os.Stat(filepath.Join(dir, marker))
		if err == nil {
			return true, nil
		}
		if !os.IsNotExist(err) {
			return false, err
		}
	}
	return false, nil
}

func (p *Provider) StartCommandHelp() string {
	return "Clojure runs the uberjar from target/ on a jlink'd JRE; the JVM flags are load-bearing, not tuning."
}

func (p *Provider) Plan(dir string, _ plan.Arch) (*plan.Plan, error) {
	jdk := defaultJDK
	if v, ok := versions.Read(dir).Major("java"); ok {
		jdk = v
	}

	// One stage, unlike the example's two: its build runs on $BUILDPLATFORM
	// and only the jlink'd runtime on $TARGETPLATFORM, which is an
	// optimisation for cross-arch builds. The clojure image is itself a
	// temurin JDK, so it carries both the tools-deps CLI and jlink and can
	// do the whole job in one place.
	//
	// jlink is what makes this fit: a full JRE would be far too large for an
	// image that is resident twice at boot (embedded in the kernel and
	// unpacked into ramfs), so only the modules the server actually needs
	// are linked in.
	// Two images, exactly as the example does it: the uberjar is built by
	// the official clojure image (a pinned tools-deps CLI) and the JRE is
	// jlinked in a pristine eclipse-temurin JDK. The JRE jlink emits is what
	// the guest executes, and the CLI decides what goes in the jar, so
	// collapsing these into one image changes both.
	builder := `set -eu
export CLJ_CONFIG=/tmp/clj GITLIBS=/tmp/gitlibs
clojure -P
clojure -P -T:build
clojure -T:build uber
mkdir -p /out/stage
cp target/server.jar /out/stage/server.jar
`

	script := `set -eu
mkdir -p /out/rootfs/usr/src /out/rootfs/tmp
cp /stage/server.jar /out/rootfs/usr/src/server.jar

jlink --add-modules java.base,jdk.httpserver,java.logging,java.sql \
      --strip-debug --no-man-pages --no-header-files --compress=zip-6 \
      --output /out/rootfs/opt/jre

# Resolve what the jlink'd JRE needs, skipping the copies already under
# /out/rootfs so ldd is asked about system libraries only.
find /out/rootfs/opt/jre -type f \( -perm -u+x -o -name '*.so' \) -print \
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

	return &plan.Plan{
		Name:          "clojure",
		Provider:      p.Name(),
		BuilderImage:  "clojure:temurin-" + jdk + "-tools-deps-bookworm-slim",
		BuilderScript: builder,
		BuildImage:    "eclipse-temurin:" + jdk + "-jdk",
		Script:        script,
		Kconfig: map[string]string{
			// Replaces the base entry: libjvm.so and friends live under the
			// jlink'd JRE, not in the system library path.
			"CONFIG_LIBPOSIX_ENVIRON_ENVP1": `"LD_LIBRARY_PATH=/opt/jre/lib/server:/opt/jre/lib:/usr/local/lib:/usr/lib:/lib"`,
		},
		// The JVM flags are load-bearing rather than tuning: the guest is a
		// single CPU with no cgroup to read limits from and no perf-data
		// file to write, so the JVM has to be told all of that explicitly.
		Cmd: []string{
			"/opt/jre/bin/java",
			"-XX:+UseSerialGC",
			"-XX:ActiveProcessorCount=1",
			"-XX:-UseContainerSupport",
			"-XX:-UsePerfData",
			"-XX:TieredStopAtLevel=1",
			"-Xmx256m",
			"-jar", "/usr/src/server.jar",
		},
	}, nil
}
