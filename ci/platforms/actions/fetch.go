package actions

// Fetching action.yml happens at plan time, host-side: the runner needs to
// know what an action *is* (node? composite? docker?) before it can lay
// out steps, and raw.githubusercontent.com serves the one file that says
// so. Fetches are cached on disk keyed by slug+ref — an action at a tag is
// immutable enough for a local runner, and the cache is what keeps replans
// instant and offline-tolerant.

import (
	"fmt"
	"io"
	"net/http"
	"os"
	"path/filepath"
	"strings"
	"time"
)

// Fetcher resolves a ref to its action.yml bytes. Package-level and
// swappable so tests inject fixtures instead of the network.
type Fetcher func(ref Ref) ([]byte, error)

// FetchFunc is the active fetcher.
var FetchFunc Fetcher = httpFetch

// Fetch resolves and parses an action's metadata.
func Fetch(ref Ref) (*Metadata, error) {
	data, err := FetchFunc(ref)
	if err != nil {
		return nil, err
	}
	return parseMetadata(data)
}

func cacheDir() string {
	base, err := os.UserCacheDir()
	if err != nil {
		base = os.TempDir()
	}
	return filepath.Join(base, "bsdkrun", "gha-actions")
}

func httpFetch(ref Ref) ([]byte, error) {
	cache := filepath.Join(cacheDir(),
		sanitizeRef(ref.Owner+"-"+ref.Repo+"-"+strings.ReplaceAll(ref.Path, "/", "-")+"-"+ref.Ref)+".yml")
	if data, err := os.ReadFile(cache); err == nil {
		return data, nil
	}

	client := &http.Client{Timeout: 15 * time.Second}
	base := "https://raw.githubusercontent.com/" + ref.Owner + "/" + ref.Repo + "/" + ref.Ref + "/"
	if ref.Path != "" {
		base += ref.Path + "/"
	}
	var lastErr error
	for _, name := range []string{"action.yml", "action.yaml"} {
		resp, err := client.Get(base + name)
		if err != nil {
			lastErr = err
			continue
		}
		body, err := io.ReadAll(resp.Body)
		resp.Body.Close()
		if err != nil {
			lastErr = err
			continue
		}
		if resp.StatusCode == 200 {
			_ = os.MkdirAll(filepath.Dir(cache), 0o755)
			_ = os.WriteFile(cache, body, 0o644)
			return body, nil
		}
		lastErr = fmt.Errorf("%s%s: HTTP %d", base, name, resp.StatusCode)
	}
	return nil, lastErr
}
