package actions

// The Actions environment protocol, made durable across separate exec
// sessions. On the real runner every step of a job shares one runner
// process; here each step is its own shell, so persistence lives in files:
//
//   /tangled/.gha/env.sh   accumulated `export`s from every GITHUB_ENV
//   /tangled/.gha/path     accumulated GITHUB_PATH lines
//   /tangled/.gha/outputs/<step-id>/<name>   one file per output
//
// WrapStep gives EVERY step of a job — plain `run:` steps included, since
// GitHub's semantics say a run step sees what setup steps exported — a
// preamble that sources the accumulated state and fresh command files, and
// a postamble that folds this step's command files back in. The heredoc
// (`KEY<<EOF`) spelling of GITHUB_ENV is handled, because that is the
// spelling real setup actions use.

import (
	"fmt"
	"strings"
)

const ghaDir = "/tangled/.gha"

// preamble: restore accumulated env/PATH, open fresh command files.
const preamble = `mkdir -p ` + ghaDir + `/outputs ` + ghaDir + `/toolcache /tmp
[ -f ` + ghaDir + `/env.sh ] && . ` + ghaDir + `/env.sh
[ -f ` + ghaDir + `/path ] && while IFS= read -r p; do case ":$PATH:" in *":$p:"*) ;; *) PATH="$p:$PATH" ;; esac; done < ` + ghaDir + `/path
export PATH
export RUNNER_OS=Linux RUNNER_TEMP=/tmp RUNNER_TOOL_CACHE=` + ghaDir + `/toolcache
case "$(uname -m)" in x86_64) export RUNNER_ARCH=X64 ;; aarch64|arm64) export RUNNER_ARCH=ARM64 ;; esac
export GITHUB_ENV=` + ghaDir + `/tmp_env GITHUB_PATH=` + ghaDir + `/tmp_path
export GITHUB_OUTPUT=` + ghaDir + `/tmp_output GITHUB_STATE=` + ghaDir + `/tmp_state
export GITHUB_STEP_SUMMARY=` + ghaDir + `/tmp_summary
: > "$GITHUB_ENV"; : > "$GITHUB_PATH"; : > "$GITHUB_OUTPUT"; : > "$GITHUB_STATE"; : > "$GITHUB_STEP_SUMMARY"
`

// postamble folds the step's command files into the durable state. Runs
// even when the step body failed — the real runner also processes command
// files for failed steps — but preserves the body's exit code.
func postamble(stepID string) string {
	outDir := ghaDir + "/outputs/" + sanitizeRef(stepID)
	return `__bsdkrun_rc=$?
__bsdkrun_fold() {
  # KEY=VALUE lines and KEY<<DELIM heredocs, the two GITHUB_ENV spellings.
  __delim=""; __key=""
  while IFS= read -r line || [ -n "$line" ]; do
    if [ -n "$__delim" ]; then
      if [ "$line" = "$__delim" ]; then
        __delim=""; __key=""
      else
        printf '%s\n' "$line" >> "$2/$__key.tmp"
      fi
      continue
    fi
    case "$line" in
      *"<<"*) __key="${line%%<<*}"; __delim="${line#*<<}"; : > "$2/$__key.tmp" ;;
      *=*) printf '%s\n' "$line" >> "$1" ;;
    esac
  done
}
mkdir -p ` + shellQuote(outDir) + ` ` + ghaDir + `/kv
if [ -s "$GITHUB_ENV" ]; then
  __bsdkrun_fold < "$GITHUB_ENV" ` + ghaDir + `/kv/env.flat ` + ghaDir + `/kv
  if [ -f ` + ghaDir + `/kv/env.flat ]; then
    while IFS= read -r kv; do
      k="${kv%%=*}"; v="${kv#*=}"
      printf 'export %s=%s\n' "$k" "'$(printf "%s" "$v" | sed "s/'/'\\\\''/g")'" >> ` + ghaDir + `/env.sh
    done < ` + ghaDir + `/kv/env.flat
    rm -f ` + ghaDir + `/kv/env.flat
  fi
  for f in ` + ghaDir + `/kv/*.tmp; do
    [ -f "$f" ] || continue
    k="$(basename "$f" .tmp)"
    printf 'export %s=%s\n' "$k" "'$(sed "s/'/'\\\\''/g" "$f")'" >> ` + ghaDir + `/env.sh
    rm -f "$f"
  done
fi
[ -s "$GITHUB_PATH" ] && cat "$GITHUB_PATH" >> ` + ghaDir + `/path
if [ -s "$GITHUB_OUTPUT" ]; then
  __bsdkrun_fold < "$GITHUB_OUTPUT" ` + ghaDir + `/kv/out.flat ` + ghaDir + `/kv
  if [ -f ` + ghaDir + `/kv/out.flat ]; then
    while IFS= read -r kv; do
      printf '%s' "${kv#*=}" > ` + shellQuote(outDir) + `"/${kv%%=*}"
    done < ` + ghaDir + `/kv/out.flat
    rm -f ` + ghaDir + `/kv/out.flat
  fi
  for f in ` + ghaDir + `/kv/*.tmp; do
    [ -f "$f" ] || continue
    mv "$f" ` + shellQuote(outDir) + `"/$(basename "$f" .tmp)"
  done
fi
exit $__bsdkrun_rc`
}

// WrapStep applies the protocol around a step body.
func WrapStep(stepID, body string) string {
	return preamble + "\n{\n" + body + "\n}\n" + postamble(stepID)
}

// NodeProvisionStep installs a node runtime once per VM, for JavaScript
// actions. The official tarball, distro-neutral, arch-aware — and a no-op
// when the image already carries node.
func NodeProvisionStep() Step {
	return Step{
		Name: "Provision actions runtime (node)",
		Command: `command -v node >/dev/null 2>&1 && { echo "node $(node --version) already present"; exit 0; }
if command -v apk >/dev/null 2>&1; then
  # Official tarballs are glibc; on musl the distro package is the one
  # that actually runs.
  apk add --no-cache nodejs
  node --version
  exit 0
fi
command -v curl >/dev/null 2>&1 || {
  apt-get update -qq && apt-get install -y -qq --no-install-recommends curl ca-certificates xz-utils
}
case "$(uname -m)" in x86_64) a=x64 ;; aarch64|arm64) a=arm64 ;; *) echo "unsupported arch"; exit 1 ;; esac
v=$(curl -fsSL https://nodejs.org/dist/latest-v24.x/ | grep -oE 'node-v24[0-9.]*-linux-'"$a"'\.tar\.xz' | head -1)
[ -n "$v" ] || { echo "could not resolve a node 24 tarball"; exit 1; }
curl -fsSL "https://nodejs.org/dist/latest-v24.x/$v" | tar -xJ -C /usr/local --strip-components=1
node --version`,
	}
}

// Fingerprint helps tests assert wrapping happened without matching the
// whole scripts.
func Fingerprint() (string, string) {
	return preamble[:40], fmt.Sprintf("outputs dir under %s", ghaDir)
}

var _ = strings.TrimSpace
