// Package scala builds sbt projects.
//
// Gradle and Maven builds of Scala go through the java provider — what
// decides the build is the tool, not the language. This provider is for
// build.sbt.
package scala

import (
	"os"
	"path/filepath"

	"github.com/tsirysndr/bsdkrun/pack/internal/plan"
	"github.com/tsirysndr/bsdkrun/pack/internal/providers/jvm"
	"github.com/tsirysndr/bsdkrun/pack/internal/versions"
)

const (
	defaultJDK = "21"
	// sbtVersion is the launcher, not the build's own sbt version — that
	// comes from project/build.properties, which the launcher reads and
	// honours. Pinned to a release tarball rather than an image tag because
	// the official sbt images encode the JDK, sbt and Scala versions in one
	// tag, so any of the three moving breaks the reference.
	sbtVersion = "1.10.7"
	jarName    = "server.jar"
)

type Provider struct{}

func New() *Provider { return &Provider{} }

func (p *Provider) Name() string { return "scala" }

func (p *Provider) Detect(dir string) (bool, error) {
	_, err := os.Stat(filepath.Join(dir, "build.sbt"))
	if err == nil {
		return true, nil
	}
	if !os.IsNotExist(err) {
		return false, err
	}
	return false, nil
}

func (p *Provider) StartCommandHelp() string {
	return "Scala runs the sbt-assembly jar on a jlink'd JRE. The project needs sbt-assembly in project/plugins.sbt."
}

func (p *Provider) Plan(dir string, _ plan.Arch) (*plan.Plan, error) {
	jdk := defaultJDK
	if v, ok := versions.Read(dir).Major("java"); ok {
		jdk = v
	}

	// `sbt assembly` rather than `sbt package`: package emits a jar holding
	// this project's classes only, without so much as the Scala standard
	// library, and a guest running it dies on the first Scala symbol it
	// touches. assembly comes from a plugin, so it is tried first and the
	// failure explains itself rather than surfacing as NoClassDefFoundError
	// in the guest.
	builder := `set -eu
apt-get update -qq
apt-get install -y -qq --no-install-recommends curl ca-certificates >/dev/null
curl -fsSL "https://github.com/sbt/sbt/releases/download/v` + sbtVersion + `/sbt-` + sbtVersion + `.tgz" \
  | tar -xz -C /opt
export PATH=/opt/sbt/bin:$PATH
export COURSIER_CACHE=/tmp/coursier SBT_OPTS="-Dsbt.global.base=/tmp/sbt -Dsbt.ivy.home=/tmp/ivy"

if ! sbt -batch assembly; then
    echo "sbt assembly failed. A unikernel needs one jar with every dependency in it;" >&2
    echo "add sbt-assembly to project/plugins.sbt:" >&2
    echo '  addSbtPlugin("com.eed3si9n" % "sbt-assembly" % "2.2.0")' >&2
    exit 1
fi

jar=$(ls -S target/scala-*/*assembly*.jar 2>/dev/null | head -1)
if [ -z "$jar" ]; then
    echo "sbt assembly produced no jar under target/scala-*/" >&2
    exit 1
fi
mkdir -p /out/stage
cp "$jar" /out/stage/` + jarName + `
`

	return &plan.Plan{
		Name:          "scala",
		Provider:      p.Name(),
		BuilderImage:  "eclipse-temurin:" + jdk + "-jdk",
		BuilderScript: builder,
		BuildImage:    "eclipse-temurin:" + jdk + "-jdk",
		Script:        jvm.Runtime(jarName),
		Kconfig:       jvm.LibraryPath(),
		Cmd:           jvm.Command("/usr/src/" + jarName),
	}, nil
}
