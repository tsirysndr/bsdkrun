package main

// Secrets for local CI runs. Spindle injects a repository's vault secrets as
// environment variables into every step and masks their values in logs; a
// local run has no vault, so the values come from the operator instead:
//
//   --secret KEY=VALUE          explicit value
//   --secret KEY                value read from the host environment
//   --secrets-file <path>       dotenv file (KEY=VALUE per line), repeatable
//   .tangled/secrets.env        auto-loaded from the workflow root when
//                               present — keep it gitignored; the clone step
//                               would otherwise ship it into the guest
//
// Precedence: pipeline env < workflow `environment:` < secrets < step env.
// A secret is run-time input, so it beats what the committed workflow says —
// with the file-level exception that a step's own environment stays the most
// specific thing in the file.
//
// Masking mirrors spindle's models.SecretMask (not imported — that package
// drags in go-git and indigo for a 50-line masker): every secret value is
// replaced by `***` in all emitted output, including its base64 and
// unpadded-base64 encodings, so `echo $TOKEN | base64` leaks nothing either.

import (
	"encoding/base64"
	"fmt"
	"os"
	"strings"
)

// secretsFileName is auto-loaded from the workflow root when present.
const secretsFileName = ".tangled/secrets.env"

// collectSecrets resolves every secret source into one map. `flags` are the
// --secret values, `files` the --secrets-file paths; `root` is the workflow
// root probed for the well-known file. Later sources win: file defaults,
// then explicit files, then flags.
func collectSecrets(root string, flags, files []string) (map[string]string, error) {
	out := map[string]string{}

	wellKnown := root + "/" + secretsFileName
	if _, err := os.Stat(wellKnown); err == nil {
		if err := readEnvFile(wellKnown, out); err != nil {
			return nil, err
		}
		fmt.Fprintf(os.Stderr, "loaded secrets from %s\n", wellKnown)
	}
	// UIs hand secrets through the environment rather than argv — argv is
	// world-readable in `ps`, the environment of a child is not.
	if raw := os.Getenv("BSDKRUN_CI_SECRETS"); raw != "" {
		if err := parseEnvContent(raw, "$BSDKRUN_CI_SECRETS", out); err != nil {
			return nil, err
		}
	}
	for _, f := range files {
		if err := readEnvFile(f, out); err != nil {
			return nil, err
		}
	}
	for _, kv := range flags {
		k, v, ok := strings.Cut(kv, "=")
		if k == "" {
			return nil, fmt.Errorf("--secret wants KEY=VALUE or KEY, got %q", kv)
		}
		if !ok {
			// Bare KEY: pass the host's own value through. Missing is an
			// error, not an empty string — an empty secret that *looks*
			// injected fails somewhere far less legible than here.
			hostVal, present := os.LookupEnv(k)
			if !present {
				return nil, fmt.Errorf("--secret %s: not set in the environment", k)
			}
			v = hostVal
		}
		out[k] = v
	}
	return out, nil
}

// readEnvFile parses dotenv-style lines (KEY=VALUE, # comments, optional
// `export ` prefix, optional single/double quotes) into dst.
func readEnvFile(path string, dst map[string]string) error {
	data, err := os.ReadFile(path)
	if err != nil {
		return fmt.Errorf("reading %s: %w", path, err)
	}
	return parseEnvContent(string(data), path, dst)
}

func parseEnvContent(content, source string, dst map[string]string) error {
	for i, raw := range strings.Split(content, "\n") {
		line := strings.TrimSpace(raw)
		if line == "" || strings.HasPrefix(line, "#") {
			continue
		}
		line = strings.TrimPrefix(line, "export ")
		k, v, ok := strings.Cut(line, "=")
		k = strings.TrimSpace(k)
		if !ok || k == "" {
			return fmt.Errorf("%s:%d: not KEY=VALUE: %q", source, i+1, raw)
		}
		v = strings.TrimSpace(v)
		if len(v) >= 2 {
			if (v[0] == '"' && v[len(v)-1] == '"') || (v[0] == '\'' && v[len(v)-1] == '\'') {
				v = v[1 : len(v)-1]
			}
		}
		dst[k] = v
	}
	return nil
}

// newMasker builds the log masker for these secret values, or nil when there
// is nothing to hide (callers treat nil as identity).
func newMasker(secrets map[string]string) *strings.Replacer {
	var pairs []string
	for _, v := range secrets {
		if v == "" {
			continue
		}
		pairs = append(pairs, v, "***")
		b64 := base64.StdEncoding.EncodeToString([]byte(v))
		if b64 != v {
			pairs = append(pairs, b64, "***")
		}
		if noPad := strings.TrimRight(b64, "="); noPad != b64 && noPad != v {
			pairs = append(pairs, noPad, "***")
		}
	}
	if len(pairs) == 0 {
		return nil
	}
	return strings.NewReplacer(pairs...)
}
