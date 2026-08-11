package kraftfile

import (
	"strings"
	"testing"

	"github.com/tsirysndr/bsdkrun/pack/internal/plan"
)

func TestGenerateStrace(t *testing.T) {
	p := &plan.Plan{Name: "hello", Provider: "go", Cmd: []string{"/hello"}}

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

// A provider overriding a base symbol must leave exactly one occurrence in
// the rendered Kraftfile — duplicates are last-wins at best, a parse error
// at worst.
func TestProviderOverrideReplacesBaseEntry(t *testing.T) {
	p := &plan.Plan{
		Name: "app", Provider: "clojure", Cmd: []string{"/x"},
		Kconfig: map[string]string{
			"CONFIG_LIBPOSIX_ENVIRON_ENVP1": `"LD_LIBRARY_PATH=/opt/jre/lib/server"`,
		},
	}
	out, err := Generate(p, Options{})
	if err != nil {
		t.Fatal(err)
	}
	if n := strings.Count(out, "CONFIG_LIBPOSIX_ENVIRON_ENVP1:"); n != 1 {
		t.Errorf("ENVP1 appears %d times, want exactly 1:\n%s", n, out)
	}
	if !strings.Contains(out, "/opt/jre/lib/server") {
		t.Error("the provider's value must be the one that survives")
	}
}
