// Package java builds Maven and Gradle projects.
//
// Scala's sbt projects have their own provider; Gradle and Maven builds of
// Scala, Kotlin or Groovy come through here, since what matters at this
// level is the build tool rather than the language.
package java

import (
	"os"
	"path/filepath"

	"github.com/tsirysndr/bsdkrun/pack/internal/plan"
	"github.com/tsirysndr/bsdkrun/pack/internal/providers/jvm"
	"github.com/tsirysndr/bsdkrun/pack/internal/versions"
)

const defaultJDK = "21"

// jarName is what the builder stage hands to the runtime stage. The build
// tools disagree about where output lands and what it is called, so the jar
// is renamed once, here, and everything downstream can stop caring.
const jarName = "server.jar"

type Provider struct{}

func New() *Provider { return &Provider{} }

func (p *Provider) Name() string { return "java" }

// Maven's POM can be written in any of its polyglot dialects, not only XML.
var poms = []string{
	"pom.xml", "pom.atom", "pom.clj", "pom.groovy",
	"pom.rb", "pom.scala", "pom.yaml", "pom.yml",
}

func (p *Provider) Detect(dir string) (bool, error) {
	for _, marker := range append(append([]string{}, poms...), "gradlew") {
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
	return "Java runs the built jar on a jlink'd JRE; the JVM flags are load-bearing, not tuning."
}

func (p *Provider) Plan(dir string, _ plan.Arch) (*plan.Plan, error) {
	jdk := defaultJDK
	if v, ok := versions.Read(dir).Major("java"); ok {
		jdk = v
	}

	gradle := usesGradle(dir)

	// Both tools are given a build directory inside the project and a cache
	// under /tmp. Left alone they write to $HOME, which is not where the
	// BuildKit cache mount is.
	build := `mvn -B -q -DskipTests package`
	outputs := `target/*.jar`
	image := "maven:3.9-eclipse-temurin-" + jdk
	if gradle {
		build = `chmod +x ./gradlew
./gradlew --no-daemon --console=plain build -x test`
		outputs = `build/libs/*.jar`
		image = "eclipse-temurin:" + jdk + "-jdk"
	}

	// The largest jar is the fat one. Both tools leave a thin jar beside it
	// (Maven's shade plugin keeps the original as original-*.jar, Gradle's
	// shadow plugin emits both a -plain and an -all), and picking the thin
	// one produces a guest that starts and then cannot find its own
	// dependencies.
	builder := `set -eu
export GRADLE_USER_HOME=/tmp/gradle MAVEN_OPTS=-Dmaven.repo.local=/tmp/m2
` + build + `

jar=$(ls -S ` + outputs + ` 2>/dev/null | grep -v -- '-plain\.jar$' | grep -v '^.*original-' | head -1)
if [ -z "$jar" ]; then
    echo "no jar found in ` + outputs + ` -- does this project build an executable jar?" >&2
    exit 1
fi
mkdir -p /out/stage
cp "$jar" /out/stage/` + jarName + `
`

	return &plan.Plan{
		Name:          "java",
		Provider:      p.Name(),
		BuilderImage:  image,
		BuilderScript: builder,
		BuildImage:    "eclipse-temurin:" + jdk + "-jdk",
		Script:        jvm.Runtime(jarName),
		Kconfig:       jvm.LibraryPath(),
		Cmd:           jvm.Command("/usr/src/" + jarName),
	}, nil
}

// usesGradle reports whether the project builds with Gradle. A project
// carrying both a wrapper and a POM is rare but real (a Maven build kept for
// publishing, say); the wrapper is the one that was committed deliberately.
func usesGradle(dir string) bool {
	_, err := os.Stat(filepath.Join(dir, "gradlew"))
	return err == nil
}
