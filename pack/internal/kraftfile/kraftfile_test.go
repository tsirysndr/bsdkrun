package kraftfile

import (
	"strings"
	"testing"

	"github.com/tsirysndr/bsdkrun/pack/internal/detect"
	"github.com/tsirysndr/bsdkrun/pack/internal/plan"
)

func TestGenerateStrace(t *testing.T) {
	p := &plan.Plan{Name: "hello", Provider: detect.Go, Cmd: []string{"/hello"}}

	off, err := Generate(p, Options{Strace: false})
	if err != nil {
		t.Fatal(err)
	}
	if !strings.Contains(off, "CONFIG_LIBSYSCALL_SHIM_STRACE: 'n'") {
		t.Errorf("Strace: false should render 'n', got:\n%s", off)
	}

	on, err := Generate(p, Options{Strace: true})
	if err != nil {
		t.Fatal(err)
	}
	if !strings.Contains(on, "CONFIG_LIBSYSCALL_SHIM_STRACE: 'y'") {
		t.Errorf("Strace: true should render 'y', got:\n%s", on)
	}
}
