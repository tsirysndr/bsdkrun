package plan

import (
	"os"
	"path/filepath"
	"strings"
	"testing"

	"github.com/tsirysndr/bsdkrun/pack/internal/detect"
)

func goProject(t *testing.T) string {
	t.Helper()
	dir := t.TempDir()
	if err := os.WriteFile(filepath.Join(dir, "go.mod"),
		[]byte("module example.com/acme/widget\n\ngo 1.22\n"), 0o644); err != nil {
		t.Fatal(err)
	}
	return dir
}

// The amd64 binary must be linked above the Unikraft kernel image: `go
// build` emits a non-PIE ET_EXEC pinned at 0x400000 (4 MiB), and the fc
// kernel lives at ~1 MiB and grows past that with the rootfs embedded, so
// the loader maps the app over the running kernel and dies. arm64 links at
// 0x10000 with the kernel 2 GiB away and has always worked, so it must be
// left alone.
func TestGoPlanRelocatesAmd64Only(t *testing.T) {
	dir := goProject(t)

	amd64, err := Build(&detect.Detection{Provider: detect.Go, Dir: dir}, ArchAmd64)
	if err != nil {
		t.Fatal(err)
	}
	if !strings.Contains(amd64.Script, "-T "+amd64TextAddr) {
		t.Errorf("amd64 script should link at %s, got:\n%s", amd64TextAddr, amd64.Script)
	}

	arm64, err := Build(&detect.Detection{Provider: detect.Go, Dir: dir}, ArchArm64)
	if err != nil {
		t.Fatal(err)
	}
	if strings.Contains(arm64.Script, "-T") {
		t.Errorf("arm64 must keep its proven default layout, got:\n%s", arm64.Script)
	}

	// Both still name the binary after the module's last path element.
	for _, p := range []*Plan{amd64, arm64} {
		if p.Name != "widget" {
			t.Errorf("Name = %q, want %q", p.Name, "widget")
		}
		if len(p.Cmd) != 1 || p.Cmd[0] != "/widget" {
			t.Errorf("Cmd = %v, want [/widget]", p.Cmd)
		}
	}
}
