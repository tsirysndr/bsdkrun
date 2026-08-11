// Package jvm holds what every JVM-language provider here needs: a runtime
// small enough to fit in a unikernel, and the flags that make a JVM behave
// on one.
//
// Clojure, Java and Scala differ only in how they get from source to a jar.
// Everything after that — jlink the runtime, resolve its libraries, tell the
// JVM it is alone on one CPU — is identical, and is here so it cannot drift
// between them.
package jvm

// Flags are the JVM options the guest needs. These are load-bearing rather
// than tuning: the guest is a single CPU with no cgroup to read limits from
// and no perf-data file to write, so the JVM has to be told all of it
// explicitly. Left to its own devices it sizes a GC thread pool from a CPU
// count it cannot determine and dies before main().
func Flags() []string {
	return []string{
		"-XX:+UseSerialGC",
		"-XX:ActiveProcessorCount=1",
		"-XX:-UseContainerSupport",
		"-XX:-UsePerfData",
		"-XX:TieredStopAtLevel=1",
		"-Xmx256m",
	}
}

// Command is the guest argv for running jar with the runtime jlink put at
// /opt/jre.
func Command(jar string) []string {
	return append(append([]string{"/opt/jre/bin/java"}, Flags()...), "-jar", jar)
}

// LibraryPath replaces the base LD_LIBRARY_PATH entry: libjvm.so and its
// neighbours live under the jlink'd runtime, not in any system directory.
func LibraryPath() map[string]string {
	return map[string]string{
		"CONFIG_LIBPOSIX_ENVIRON_ENVP1": `"LD_LIBRARY_PATH=/opt/jre/lib/server:/opt/jre/lib:/usr/local/lib:/usr/lib:/lib"`,
	}
}

// FallbackModules is what the runtime is built from when jdeps cannot work
// out the answer. It is deliberately broad — a missing module surfaces as a
// ClassNotFoundException at runtime, in a guest with no way to install one.
const FallbackModules = "java.base,java.logging,java.sql,java.naming,java.management," +
	"java.instrument,java.desktop,java.security.jgss,jdk.httpserver,jdk.unsupported,jdk.crypto.ec"

// Runtime is the second-stage script shared by every JVM provider. It takes
// the jar the builder stage left at /stage/<jar>, links a runtime sized to
// it, and resolves that runtime's libraries into the rootfs.
//
// The module set comes from jdeps where possible. Hardcoding it would be
// simpler, but a jar's needs depend on its dependencies — a framework
// reaching for java.naming or java.management is ordinary — and the failure
// mode for guessing low is a ClassNotFoundException in a guest that cannot
// be fixed without a rebuild. jdeps is asked, and its answer is used only if
// it succeeds; --ignore-missing-deps keeps optional dependencies from
// failing the analysis outright.
func Runtime(jar string) string {
	return `set -eu
mkdir -p /out/rootfs/usr/src /out/rootfs/tmp
cp /stage/` + jar + ` /out/rootfs/usr/src/` + jar + `

modules=$(jdeps --print-module-deps --ignore-missing-deps --multi-release base \
              /out/rootfs/usr/src/` + jar + ` 2>/dev/null || true)
if [ -z "$modules" ]; then
    modules=` + FallbackModules + `
fi
# jdeps reports only what the bytecode references. Anything reached by
# reflection -- a JDBC driver, a logging backend, a crypto provider -- is
# invisible to it, so the fallback set is unioned in rather than replaced.
modules="$modules,` + FallbackModules + `"

jlink --add-modules "$modules" \
      --strip-debug --no-man-pages --no-header-files --compress=zip-6 \
      --output /out/rootfs/opt/jre

# Resolve what the jlink'd runtime needs, skipping the copies already under
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
}
