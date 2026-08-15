package bsdkrun

import "encoding/json"

// Compression is an archive format a cache entry can be stored in.
type Compression string

const (
	Gzip          Compression = "gzip"
	Zstd          Compression = "zstd"
	Estargz       Compression = "estargz"
	NoCompression Compression = "none"
)

// CacheEntry is a stored cache entry, as `cache ls` reports it.
type CacheEntry struct {
	Key string `json:"key"`
	// Path is the guest directory the tree came from.
	Path        string      `json:"path"`
	Compression Compression `json:"compression"`
	// Size of the archive in bytes.
	Size int64 `json:"size"`
	// Created is unix seconds.
	Created int64 `json:"created"`
	// Digest is `sha256:…` over the archive.
	Digest string `json:"digest"`
}

// RestoreResult is what a restore did. A miss is not an error — check Restored.
type RestoreResult struct {
	Restored bool `json:"restored"`
	// RequestedKey is the key that was asked for.
	RequestedKey string `json:"requested_key"`
	// Key is the entry actually used. It differs from RequestedKey when a
	// RestoreKeys prefix matched, and is empty on a miss.
	Key         string      `json:"key"`
	Path        string      `json:"path"`
	Size        int64       `json:"size"`
	Compression Compression `json:"compression"`
	Created     int64       `json:"created"`
}

// SaveOptions tunes Cache.Save.
type SaveOptions struct {
	// Key to store under. Make it name the content — a lockfile hash.
	Key string
	// Compression defaults to gzip.
	Compression Compression
	// Force replaces an entry that already has this key.
	Force bool
}

// RestoreOptions tunes Cache.Restore.
type RestoreOptions struct {
	Key string
	// Path defaults to the directory the entry was saved from.
	Path string
	// RestoreKeys are prefixes tried in order when Key misses; within a prefix
	// the newest matching entry wins.
	RestoreKeys []string
}

// Cache saves and restores guest directories under a key, so a rebuild can pick
// up where the last one left off. Reach it through Sandbox.Cache.
//
//	hit, _ := box.Cache().Restore(bsdkrun.RestoreOptions{Key: key, RestoreKeys: []string{"deps-"}})
//	if !hit.Restored {
//	    box.Exec("npm", "ci")
//	    box.Cache().Save("/app/node_modules", bsdkrun.SaveOptions{Key: key})
//	}
//
// Where entries live — host disk or S3 — is host configuration, not an SDK
// concern: set BSDKRUN_CACHE_BACKEND / BSDKRUN_CACHE_S3_*, or write
// ~/.config/bsdkrun/cache.toml.
type Cache struct {
	id string
}

// Cache returns a handle to this machine's keyed directory cache.
func (s *Sandbox) Cache() *Cache { return &Cache{id: s.ID} }

// Save archives the guest directory at path under opts.Key.
func (c *Cache) Save(path string, opts SaveOptions) (*CacheEntry, error) {
	args := []string{"cache", "save", c.id + ":" + path, "--key", opts.Key, "--json"}
	if opts.Compression != "" && opts.Compression != Gzip {
		args = append(args, "--compression", string(opts.Compression))
	}
	if opts.Force {
		args = append(args, "--force")
	}
	out, err := cacheJSON(args, "bsdkrun cache save")
	if err != nil {
		return nil, err
	}
	var entry CacheEntry
	if err := json.Unmarshal(out, &entry); err != nil {
		return nil, err
	}
	return &entry, nil
}

// Restore puts a stored tree back. A miss is reported through
// RestoreResult.Restored, not as an error.
func (c *Cache) Restore(opts RestoreOptions) (*RestoreResult, error) {
	target := c.id
	if opts.Path != "" {
		target = c.id + ":" + opts.Path
	}
	args := []string{"cache", "restore", target, "--key", opts.Key, "--json"}
	if len(opts.RestoreKeys) > 0 {
		args = append(args, "--restore-keys")
		args = append(args, opts.RestoreKeys...)
	}
	out, err := cacheJSON(args, "bsdkrun cache restore")
	if err != nil {
		return nil, err
	}
	var result RestoreResult
	if err := json.Unmarshal(out, &result); err != nil {
		return nil, err
	}
	return &result, nil
}

// ListCaches returns every stored cache entry, newest first.
func ListCaches() ([]CacheEntry, error) {
	out, err := cacheJSON([]string{"cache", "ls", "--json"}, "bsdkrun cache ls")
	if err != nil {
		return nil, err
	}
	var entries []CacheEntry
	if err := json.Unmarshal(out, &entries); err != nil {
		return nil, err
	}
	return entries, nil
}

// RemoveCache removes entries by key. With all set, it removes every entry and
// keys is ignored.
func RemoveCache(keys []string, all bool) error {
	args := []string{"cache", "rm"}
	if all {
		args = append(args, "--all")
	} else {
		args = append(args, keys...)
	}
	_, err := RunChecked(args, "bsdkrun cache rm", nil)
	return err
}

func cacheJSON(args []string, label string) ([]byte, error) {
	res, err := RunChecked(args, label, nil)
	if err != nil {
		return nil, err
	}
	if res.Stdout == "" {
		return []byte("{}"), nil
	}
	return []byte(res.Stdout), nil
}
