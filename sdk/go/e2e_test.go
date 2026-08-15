package bsdkrun

// End-to-end smoke test against a real bsdkrun binary. Off by default: it
// needs a provisioned host (libkrun, images) and boots a VM, so it only
// runs when BSDKRUN_SDK_E2E is set.

import (
	"os"
	"testing"
)

func TestE2ELinuxBootAndExec(t *testing.T) {
	if os.Getenv("BSDKRUN_SDK_E2E") == "" {
		t.Skip("set BSDKRUN_SDK_E2E=1 to run the e2e test (boots a real VM)")
	}

	if !System.Probe() {
		t.Fatal("bsdkrun probe failed; is the toolchain provisioned?")
	}

	sbx, err := Linux("alpine").Command("sleep", "300").Create()
	if err != nil {
		t.Fatal(err)
	}
	defer func() {
		if err := sbx.Remove(true); err != nil {
			t.Errorf("remove: %v", err)
		}
	}()

	res, err := sbx.Exec("uname", "-a")
	if err != nil {
		t.Fatal(err)
	}
	if err := res.Err(); err != nil {
		t.Fatal(err)
	}
	if res.Text() == "" {
		t.Fatal("uname produced no output")
	}
}
