package kraft

import "fmt"

// Arch is a kraft/Unikraft target architecture name — "x86_64", not Docker's
// "amd64", which is why this is a distinct type from buildkit.Platform
// rather than reusing it.
type Arch string

const (
	ArchArm64  Arch = "arm64"
	ArchX86_64 Arch = "x86_64"
)

// FromDockerArch maps a Docker/OCI platform arch ("amd64"/"arm64") to the
// kraft/Unikraft name used in Kraftfile `targets:` (fc/x86_64, fc/arm64) and
// in kraft's own build output path (.unikraft/build/<name>_fc-x86_64).
func FromDockerArch(dockerArch string) (Arch, error) {
	switch dockerArch {
	case "arm64":
		return ArchArm64, nil
	case "amd64":
		return ArchX86_64, nil
	default:
		return "", fmt.Errorf("unsupported architecture %q", dockerArch)
	}
}

func (a Arch) other() Arch {
	if a == ArchArm64 {
		return ArchX86_64
	}
	return ArchArm64
}

// rustTarget is the Rust target triple app-elfloader-rs (the loader kraft
// builds as part of `kraft build`) is compiled for.
func (a Arch) rustTarget() string {
	if a == ArchArm64 {
		return "aarch64-unknown-none-softfloat"
	}
	return "x86_64-unknown-none"
}

// debArch is the architecture name in kraftkit's .deb release asset name
// and in Docker's own arch reporting (arm64/amd64 — not x86_64).
func (a Arch) debArch() string {
	if a == ArchArm64 {
		return "arm64"
	}
	return "amd64"
}
