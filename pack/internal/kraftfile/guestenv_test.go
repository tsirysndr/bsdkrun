package kraftfile

import (
	"strings"
	"testing"

	"github.com/tsirysndr/bsdkrun/pack/internal/plan"
)

// A project variable must not land on an index a provider already claimed.
// beam takes ENVP4..7 and gleam 8..9; writing PORT to a fixed offset would
// silently replace ROOTDIR and break the guest in a way that looks nothing
// like a config mistake.
func TestGuestEnvSkipsProviderIndices(t *testing.T) {
	p := &plan.Plan{
		Kconfig: map[string]string{
			"CONFIG_LIBPOSIX_ENVIRON_ENVP4": `"ROOTDIR=/erl"`,
			"CONFIG_LIBPOSIX_ENVIRON_ENVP5": `"BINDIR=/erl/erts/bin"`,
		},
		GuestEnv: map[string]string{"PORT": "8080"},
	}
	applyGuestEnv(p)

	if got := p.Kconfig["CONFIG_LIBPOSIX_ENVIRON_ENVP4"]; got != `"ROOTDIR=/erl"` {
		t.Errorf("ENVP4 was overwritten: %s", got)
	}
	if got := p.Kconfig["CONFIG_LIBPOSIX_ENVIRON_ENVP6"]; got != `"PORT=8080"` {
		t.Errorf("ENVP6 = %q, want the new variable", got)
	}
}

// Overriding a variable the base block sets must replace it, not add a
// second entry — otherwise which one wins is anybody's guess.
func TestGuestEnvOverridesInPlace(t *testing.T) {
	p := &plan.Plan{
		Kconfig:  map[string]string{},
		GuestEnv: map[string]string{"HOME": "/srv"},
	}
	applyGuestEnv(p)

	if got := p.Kconfig["CONFIG_LIBPOSIX_ENVIRON_ENVP2"]; got != `"HOME=/srv"` {
		t.Errorf("HOME should replace the base ENVP2, got %q", got)
	}
	for k := range p.Kconfig {
		if k != "CONFIG_LIBPOSIX_ENVIRON_ENVP2" {
			t.Errorf("unexpected extra entry %s", k)
		}
	}
}

// Replacing a provider's own variable must reuse its index too.
func TestGuestEnvOverridesProviderVariable(t *testing.T) {
	p := &plan.Plan{
		Kconfig:  map[string]string{"CONFIG_LIBPOSIX_ENVIRON_ENVP4": `"ROOTDIR=/erl"`},
		GuestEnv: map[string]string{"ROOTDIR": "/opt/erl"},
	}
	applyGuestEnv(p)

	if got := p.Kconfig["CONFIG_LIBPOSIX_ENVIRON_ENVP4"]; got != `"ROOTDIR=/opt/erl"` {
		t.Errorf("ENVP4 = %q, want the override in place", got)
	}
	if len(p.Kconfig) != 1 {
		t.Errorf("expected one entry, got %d", len(p.Kconfig))
	}
}

// A value with a space or a quote has to survive into the Kraftfile.
func TestGuestEnvQuotes(t *testing.T) {
	p := &plan.Plan{
		Kconfig:  map[string]string{},
		GuestEnv: map[string]string{"GREETING": `hello "world"`},
	}
	applyGuestEnv(p)

	got := p.Kconfig["CONFIG_LIBPOSIX_ENVIRON_ENVP4"]
	if !strings.Contains(got, `\"world\"`) {
		t.Errorf("value not escaped: %s", got)
	}
}
