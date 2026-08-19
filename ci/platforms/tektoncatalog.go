package platforms

// Tekton catalog resolution. A `taskRef` that names a task this checkout
// does not carry is looked up in the tektoncd/catalog — the same tasks the
// hub resolver serves in a cluster. With a version (the hub resolver's
// `version` param) the task YAML comes straight from the catalog repo's
// well-known layout; without one, Artifact Hub answers with the newest
// version's manifest (the Tekton Hub API it replaced was sunset in 2024).

import (
	"encoding/json"
	"fmt"
	"io"
	"net/http"
	"time"
)

// TektonCatalogFunc fetches a catalog task's YAML by name and optional
// version. Package-level and swappable so tests inject fixtures.
var TektonCatalogFunc func(name, version string) (string, error) = fetchTektonCatalogTask

func fetchTektonCatalogTask(name, version string) (string, error) {
	client := &http.Client{Timeout: 30 * time.Second}
	if version != "" {
		url := fmt.Sprintf(
			"https://raw.githubusercontent.com/tektoncd/catalog/main/task/%s/%s/%s.yaml",
			name, version, name)
		resp, err := client.Get(url)
		if err != nil {
			return "", err
		}
		defer resp.Body.Close()
		if resp.StatusCode != 200 {
			return "", fmt.Errorf("catalog task %s@%s: HTTP %d from the catalog repo", name, version, resp.StatusCode)
		}
		data, err := io.ReadAll(io.LimitReader(resp.Body, 4<<20))
		if err != nil {
			return "", err
		}
		return string(data), nil
	}

	resp, err := client.Get("https://artifacthub.io/api/v1/packages/tekton-task/tekton-catalog-tasks/" + name)
	if err != nil {
		return "", err
	}
	defer resp.Body.Close()
	if resp.StatusCode != 200 {
		return "", fmt.Errorf("catalog task %s: HTTP %d from Artifact Hub", name, resp.StatusCode)
	}
	body, err := io.ReadAll(io.LimitReader(resp.Body, 4<<20))
	if err != nil {
		return "", err
	}
	var out struct {
		Data struct {
			ManifestRaw string `json:"manifestRaw"`
		} `json:"data"`
	}
	if err := json.Unmarshal(body, &out); err != nil {
		return "", fmt.Errorf("catalog task %s: Artifact Hub answered non-JSON", name)
	}
	if out.Data.ManifestRaw == "" {
		return "", fmt.Errorf("catalog task %s: not found on Artifact Hub", name)
	}
	return out.Data.ManifestRaw, nil
}
