package platforms

// Real Buildkite plugins. Unlike Jenkins plugins (Java inside Jenkins'
// runtime) or GitHub actions (JavaScript under a protocol), a Buildkite
// plugin is the simplest possible extension: a git repository of shell
// hooks. So the real runner is small: clone the plugin at its ref, export
// its configuration as BUILDKITE_PLUGIN_<NAME>_<KEY> (nested maps
// flattened with underscores, arrays indexed — Buildkite's own scheme),
// then run the hook lifecycle around the step's command in one shell:
// `environment` is sourced (it exists to mutate env), `pre-command` runs,
// the command runs, `post-command` runs even on failure with the command's
// exit code preserved — the agent's own ordering.
//
// A plugin that shells out to docker will fail in the guest for the same
// reason container actions are refused: a microVM runs no Docker daemon.
// That failure is the plugin's own honest output, not a silent skip.

import (
	"fmt"
	"sort"
	"strings"

	"gopkg.in/yaml.v3"
)

// bkPlugin is one parsed plugin reference plus its config.
type bkPlugin struct {
	// Name as written, e.g. "docker" or "org/name".
	Name string
	Ref  string
	// Env is the flattened BUILDKITE_PLUGIN_* map.
	Env map[string]string
}

// bkParsePlugins reads the `plugins:` node: a list whose items are either
// "name#ref" strings or {"name#ref": config} maps.
func bkParsePlugins(n yaml.Node) []bkPlugin {
	if n.IsZero() {
		return nil
	}
	var items []yaml.Node
	if err := n.Decode(&items); err != nil {
		return nil
	}
	var out []bkPlugin
	for _, item := range items {
		var plain string
		if err := item.Decode(&plain); err == nil {
			if p, ok := bkPluginRef(plain, nil); ok {
				out = append(out, p)
			}
			continue
		}
		var m map[string]yaml.Node
		if err := item.Decode(&m); err == nil {
			for spec, cfg := range m {
				var cfgVal interface{}
				_ = cfg.Decode(&cfgVal)
				if p, ok := bkPluginRef(spec, cfgVal); ok {
					out = append(out, p)
				}
			}
		}
	}
	return out
}

func bkPluginRef(spec string, cfg interface{}) (bkPlugin, bool) {
	name, ref, _ := strings.Cut(spec, "#")
	if name == "" {
		return bkPlugin{}, false
	}
	p := bkPlugin{Name: name, Ref: ref, Env: map[string]string{}}
	prefix := "BUILDKITE_PLUGIN_" + bkEnvName(name)
	bkFlatten(prefix, cfg, p.Env)
	return p, true
}

// bkEnvName: the repository basename, minus the -buildkite-plugin suffix,
// uppercased with non-alphanumerics as underscores — Buildkite's rule.
func bkEnvName(name string) string {
	base := name
	if i := strings.LastIndexByte(base, '/'); i >= 0 {
		base = base[i+1:]
	}
	base = strings.TrimSuffix(base, "-buildkite-plugin")
	return strings.Map(func(c rune) rune {
		switch {
		case c >= 'a' && c <= 'z':
			return c - ('a' - 'A')
		case c >= 'A' && c <= 'Z', c >= '0' && c <= '9':
			return c
		}
		return '_'
	}, base)
}

func bkFlatten(prefix string, v interface{}, out map[string]string) {
	switch val := v.(type) {
	case nil:
	case string:
		out[prefix] = val
	case bool:
		out[prefix] = fmt.Sprintf("%t", val)
	case int:
		out[prefix] = fmt.Sprintf("%d", val)
	case float64:
		out[prefix] = strings.TrimSuffix(fmt.Sprintf("%v", val), ".0")
	case []interface{}:
		for i, item := range val {
			bkFlatten(fmt.Sprintf("%s_%d", prefix, i), item, out)
		}
	case map[string]interface{}:
		for k, item := range val {
			bkFlatten(prefix+"_"+bkEnvName(k), item, out)
		}
	default:
		out[prefix] = fmt.Sprintf("%v", val)
	}
}

// bkCloneURL: bare names live in the buildkite-plugins org with the
// -buildkite-plugin suffix; owner/name follows the same suffix convention;
// full URLs pass through.
func (p bkPlugin) bkCloneURL() string {
	if strings.Contains(p.Name, "://") || strings.HasPrefix(p.Name, "git@") {
		return p.Name
	}
	slug := p.Name
	if !strings.Contains(slug, "/") {
		slug = "buildkite-plugins/" + slug
	}
	if !strings.HasSuffix(slug, "-buildkite-plugin") {
		slug += "-buildkite-plugin"
	}
	return "https://github.com/" + slug
}

func (p bkPlugin) cloneDir() string {
	return "/tangled/.bk/plugins/" + bkEnvName(p.Name) + "-" + sanitizeBk(p.Ref)
}

func sanitizeBk(s string) string {
	if s == "" {
		return "default"
	}
	return strings.Map(func(c rune) rune {
		switch {
		case c >= 'a' && c <= 'z', c >= 'A' && c <= 'Z', c >= '0' && c <= '9', c == '.', c == '-', c == '_':
			return c
		}
		return '-'
	}, s)
}

// bkPluginLifecycle wraps a step command in its plugins' hook lifecycle.
// Everything happens in one shell so the environment hook's exports reach
// the command, exactly as on the agent.
func bkPluginLifecycle(plugins []bkPlugin, command string) (string, map[string]string) {
	if len(plugins) == 0 {
		return command, nil
	}
	env := map[string]string{}
	var b strings.Builder

	for _, p := range plugins {
		for k, v := range p.Env {
			env[k] = v
		}
		dir := p.cloneDir()
		url := p.bkCloneURL()
		fmt.Fprintf(&b, "# plugin %s\n", p.Name)
		if p.Ref != "" {
			fmt.Fprintf(&b,
				"[ -d %[1]s ] || git clone --quiet --depth 1 --branch %[2]s %[3]s %[1]s 2>/dev/null || { git clone --quiet %[3]s %[1]s && git -C %[1]s checkout --quiet %[2]s; }\n",
				bkQuote(dir), bkQuote(p.Ref), bkQuote(url))
		} else {
			fmt.Fprintf(&b, "[ -d %[1]s ] || git clone --quiet --depth 1 %[2]s %[1]s\n", bkQuote(dir), bkQuote(url))
		}
	}
	// environment hooks mutate env — but a hook may `exec` (metahook does,
	// for every hook), which would replace this very shell if sourced
	// directly. So each runs in a subshell that exports its resulting env
	// through a capture file: a plain hook's exports propagate, an exec'ing
	// hook takes only its subshell with it, and its set -u can never leak.
	b.WriteString("__bk_envcap=$(mktemp)\n")
	for _, p := range plugins {
		fmt.Fprintf(&b,
			"if [ -f %[1]s/hooks/environment ]; then ( . %[1]s/hooks/environment; export -p > \"$__bk_envcap\" ) && . \"$__bk_envcap\" || true; fi\n",
			bkQuote(p.cloneDir()))
	}
	for _, p := range plugins {
		fmt.Fprintf(&b, "[ -f %[1]s/hooks/pre-command ] && %[1]s/hooks/pre-command || true\n", bkQuote(p.cloneDir()))
	}
	b.WriteString("__bk_rc=0\n{\n" + command + "\n} || __bk_rc=$?\n")
	fmt.Fprintf(&b, "export BUILDKITE_COMMAND_EXIT_STATUS=$__bk_rc\n")
	for _, p := range plugins {
		fmt.Fprintf(&b, "[ -f %[1]s/hooks/post-command ] && %[1]s/hooks/post-command || true\n", bkQuote(p.cloneDir()))
	}
	b.WriteString("exit $__bk_rc")
	return b.String(), env
}

func bkQuote(s string) string {
	return "'" + strings.ReplaceAll(s, "'", `'\''`) + "'"
}

// bkSortedPluginEnv is a deterministic view for announcements and tests.
func bkSortedPluginEnv(env map[string]string) []string {
	keys := make([]string, 0, len(env))
	for k := range env {
		keys = append(keys, k)
	}
	sort.Strings(keys)
	return keys
}
