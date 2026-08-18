package deploy

import "testing"

func TestDetectionPriorityAndRunnersUp(t *testing.T) {
	d, also := Detect([]string{"NPM_TOKEN", "FLY_API_TOKEN", "RAILWAY_TOKEN"})
	if d == nil || d.Platform != "railway" {
		t.Fatalf("priority order broken: %+v", d)
	}
	if len(also) != 1 || also[0] != "fly" {
		t.Fatalf("runners-up: %v", also)
	}

	if d, _ := Detect([]string{"NPM_TOKEN"}); d != nil {
		t.Fatalf("NPM_TOKEN is not a deploy token: %+v", d)
	}
}

func TestStepRendering(t *testing.T) {
	fly, _ := Detect([]string{"FLY_API_TOKEN"})
	dry := fly.Step(true)
	if dry.Command != `echo "[dry-run] would deploy to fly (FLY_API_TOKEN detected): flyctl deploy --remote-only"` {
		t.Fatalf("dry-run step: %q", dry.Command)
	}
	real := fly.Step(false)
	if real.Command != "flyctl deploy --remote-only" {
		t.Fatalf("real step: %q", real.Command)
	}
}
