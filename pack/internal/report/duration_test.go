package report

import (
	"testing"
	"time"
)

func TestFormatDuration(t *testing.T) {
	cases := []struct {
		in   time.Duration
		want string
	}{
		{0, "0.0s"},
		{-time.Second, "0.0s"},
		{320 * time.Millisecond, "0.3s"},
		{4500 * time.Millisecond, "4.5s"},
		{59949 * time.Millisecond, "59.9s"},
		{60 * time.Second, "1m00.0s"},
		{93 * time.Second, "1m33.0s"},
		{2*time.Hour + 3*time.Second, "120m03.0s"},
	}
	for _, c := range cases {
		if got := FormatDuration(c.in); got != c.want {
			t.Errorf("FormatDuration(%v) = %q, want %q", c.in, got, c.want)
		}
	}
}
