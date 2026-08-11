package tui

import (
	"strings"
	"testing"

	"github.com/charmbracelet/lipgloss"
)

// The duration has to land flush against the right edge no matter how much
// ANSI colouring either half carries — that's the whole point of measuring
// with lipgloss.Width instead of len.
func TestRightAlign(t *testing.T) {
	m := model{width: 40}
	line := m.rightAlign(styleDone.Render("✓ detect"), "4.5s", styleDetail)

	if !strings.HasSuffix(line, "\n") {
		t.Fatalf("line is not newline-terminated: %q", line)
	}
	if got := lipgloss.Width(strings.TrimSuffix(line, "\n")); got != 40 {
		t.Errorf("visible width = %d, want 40", got)
	}
}

// A line too long for the terminal keeps its duration — pushed to one space
// past the text rather than padded to a negative width (which would panic in
// strings.Repeat).
func TestRightAlignOverflow(t *testing.T) {
	m := model{width: 10}
	line := m.rightAlign(strings.Repeat("x", 30), "4.5s", styleDetail)
	if !strings.Contains(line, "x 4.5s") {
		t.Errorf("overflowing line lost its duration: %q", line)
	}
}

// Before the first tea.WindowSizeMsg, width is 0 and alignment falls back to
// 80 columns rather than collapsing every duration onto the text.
func TestRightAlignUnknownWidth(t *testing.T) {
	m := model{}
	line := m.rightAlign("detect", "4.5s", styleDetail)
	if got := lipgloss.Width(strings.TrimSuffix(line, "\n")); got != 80 {
		t.Errorf("visible width = %d, want the 80-column fallback", got)
	}
}
