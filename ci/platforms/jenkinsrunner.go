package platforms

// Real Jenkins, when translation is not enough. A Jenkins plugin is not a
// standalone Java program — it links against Jenkins core and lives inside
// its runtime, so "a small Java environment" for plugins means a small
// *Jenkins*: Jenkinsfile Runner, the project's own headless one-shot
// distribution. The guest gets a multi-arch JDK image and assembles the
// real thing at run time — the runner zip (1.3 MB of launcher), a pinned
// jenkins.war, and plugins resolved with the official plugin-installation-
// manager from a plugins.txt (the repository's own, if present, appended
// to the pipeline baseline) — then executes the Jenkinsfile inside an
// actual Jenkins: scripted pipelines, plugin steps, the CPS interpreter,
// all of it.
//
// This path engages when the structural translation cannot be faithful:
// scripted pipelines (previously a flat refusal) and declarative pipelines
// whose steps go beyond sh/echo/checkout. When everything translates, the
// fast path stays — booting Jenkins to run three sh steps would be
// ceremony, not fidelity.

import (
	"fmt"
	"sort"
	"strings"
)

const (
	jfrVersion     = "1.0-beta-32"
	jenkinsWar     = "2.462.3"
	pluginManager  = "2.13.2"
	jfrInstallDir  = "/opt/jfr"
	jenkinsWorkDir = "/opt/jenkins"
)

// jenkinsRunnerJob builds the real-Jenkins job. `reason` says why the
// translation path was not taken; `hasPluginsFile` appends the repo's own
// plugins.txt to the baseline.
func jenkinsRunnerJob(reason string, hasPluginsFile bool) Job {
	// Version-pinned baseline: the version-scoped update-center service is
	// gone (every ?version= and dynamic-stable path serves current), so
	// "latest" always tracks the newest LTS — beyond what the embedder can
	// host. Top-level pins from the war's era plus --latest false (below)
	// make dependencies resolve to their minimum requirements, which is
	// era-consistent by construction.
	pluginsSetup := `cat > /tmp/plugins-baseline.txt <<'EOF'
workflow-aggregator:600.vb_57cdd26fdd7
git:5.5.2
EOF
`
	if hasPluginsFile {
		pluginsSetup += `cat plugins.txt >> /tmp/plugins-baseline.txt
`
	}

	provision := fmt.Sprintf(`set -e
command -v curl >/dev/null 2>&1 || {
  apt-get -o Acquire::Check-Valid-Until=false update -qq
  apt-get install -y -qq --no-install-recommends curl ca-certificates unzip git
}
command -v git >/dev/null 2>&1 || {
  apt-get -o Acquire::Check-Valid-Until=false update -qq
  apt-get install -y -qq --no-install-recommends git
}
command -v unzip >/dev/null 2>&1 || {
  apt-get -o Acquire::Check-Valid-Until=false update -qq
  apt-get install -y -qq --no-install-recommends unzip
}
mkdir -p %[1]s %[2]s/plugins
echo "downloading Jenkinsfile Runner %[3]s"
curl -fsSL -o /tmp/jfr.zip https://github.com/jenkinsci/jenkinsfile-runner/releases/download/%[3]s/jenkinsfile-runner-%[3]s.zip
unzip -oq /tmp/jfr.zip -d %[1]s
chmod +x %[1]s/bin/jenkinsfile-runner 2>/dev/null || true
echo "downloading jenkins.war %[4]s"
curl -fsSL -o %[2]s/jenkins.war https://updates.jenkins.io/download/war/%[4]s/jenkins.war
# Explode the war ourselves: the runner's own extraction copies one byte
# per write syscall (measured: 34 million writes for 34 MB), which over
# virtio-fs means days. unzip writes like an adult, and -w accepts an
# exploded directory.
mkdir -p %[2]s/war
unzip -oq %[2]s/jenkins.war -d %[2]s/war
echo "downloading the plugin installation manager %[5]s"
curl -fsSL -o %[2]s/plugin-manager.jar https://github.com/jenkinsci/plugin-installation-manager-tool/releases/download/%[5]s/jenkins-plugin-manager-%[5]s.jar`,
		jfrInstallDir, jenkinsWorkDir, jfrVersion, jenkinsWar, pluginManager)

	// Plugins resolve from the war version's own update center — "latest"
	// tracks the newest LTS and demands cores this embedder cannot host
	// (jfr 1.0-beta-32 predates Jetty 12; Jenkins >= 2.479 ships it and the
	// embedder dies on a Jetty MimeTypes API that no longer exists).
	// No output-taming pipe here: `| tail` would replace the manager's
	// exit code with tail's, and a failed plugin resolution must fail the
	// step — a Jenkins booted with half its dependency graph dies later
	// with a ClassNotFound that points at nothing.
	installPlugins := pluginsSetup + fmt.Sprintf(
		`java -jar %[1]s/plugin-manager.jar --war %[1]s/jenkins.war \
  --latest false \
  --plugin-file /tmp/plugins-baseline.txt --plugin-download-directory %[1]s/plugins`,
		jenkinsWorkDir)

	run := fmt.Sprintf(
		`JFR=$(find %[1]s -name jenkinsfile-runner -type f | head -1)
[ -n "$JFR" ] || { echo "jenkinsfile-runner launcher not found"; exit 1; }
# An explicit heap: the default (a quarter of RAM) starves a real
# Jenkins into a GC spiral rather than failing honestly.
export JAVA_OPTS="-Xmx2g"
"$JFR" -w %[2]s/war -p %[2]s/plugins -f Jenkinsfile --runWorkspace "$PWD"`,
		jfrInstallDir, jenkinsWorkDir)

	return Job{
		Name:      "pipeline",
		Image:     "eclipse-temurin:17-jdk",
		MinMemMiB: 3072,
		Steps: []Step{
			{
				Name: "why real Jenkins",
				Command: fmt.Sprintf(`echo "running under Jenkinsfile Runner (a real headless Jenkins): %s"`,
					strings.ReplaceAll(reason, `"`, `'`)),
			},
			{Name: "provision Jenkinsfile Runner", Command: provision},
			{Name: "install plugins", Command: installPlugins},
			{Name: "run Jenkinsfile (real Jenkins)", Command: run},
		},
	}
}

// jenkinsNeedsRealRunner scans a *declarative* pipeline for steps beyond
// the translatable set. The returned names drive the announcement.
func jenkinsNeedsRealRunner(pipeline gvNode) []string {
	seen := map[string]bool{}
	var scanStages func(n gvNode)
	scanStages = func(n gvNode) {
		for _, st := range n.block {
			if st.name != "stage" {
				continue
			}
			for _, c := range st.block {
				switch c.name {
				case "steps":
					for _, s := range c.block {
						switch s.name {
						case "sh", "echo", "checkout", "script":
							// script { } blocks are Groovy — real runner.
							if s.name == "script" {
								seen["script"] = true
							}
						default:
							seen[s.name] = true
						}
					}
				case "parallel", "stages":
					scanStages(c)
				}
			}
		}
	}
	for _, n := range pipeline.block {
		if n.name == "stages" {
			scanStages(n)
		}
	}
	out := make([]string, 0, len(seen))
	for k := range seen {
		out = append(out, k)
	}
	sort.Strings(out)
	return out
}
