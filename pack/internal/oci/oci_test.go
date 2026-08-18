package oci

import (
	"os"
	"path/filepath"
	"testing"
)

// A reference with no registry belongs to Docker Hub, as it does everywhere
// else in the container ecosystem.
func TestDefaultsToDockerHub(t *testing.T) {
	cases := map[string]string{
		"you/app:v1":                      "index.docker.io/you/app:v1",
		"app:v1":                          "index.docker.io/library/app:v1",
		"ghcr.io/you/app:v1":              "ghcr.io/you/app:v1",
		"registry.example.com/team/app:1": "registry.example.com/team/app:1",
	}
	for in, want := range cases {
		ref, err := parseRef(in)
		if err != nil {
			t.Fatalf("parseRef(%q): %v", in, err)
		}
		if got := ref.Name(); got != want {
			t.Errorf("parseRef(%q) = %q, want %q", in, got, want)
		}
	}
}

// Localhost registries are almost always a registry:2 container with no
// certificate; requiring TLS there would mean nobody could try this without
// standing up a CA.
func TestLocalhostIsInsecure(t *testing.T) {
	for _, ref := range []string{"localhost:5000/app:v1", "127.0.0.1:5000/app:v1"} {
		if !isLocal(ref) {
			t.Errorf("isLocal(%q) = false, want true", ref)
		}
	}
	for _, ref := range []string{"ghcr.io/you/app:v1", "you/app:v1"} {
		if isLocal(ref) {
			t.Errorf("isLocal(%q) = true, want false", ref)
		}
	}
}

func TestIsReference(t *testing.T) {
	dir := t.TempDir()
	if err := os.WriteFile(filepath.Join(dir, "f"), nil, 0o644); err != nil {
		t.Fatal(err)
	}

	refs := []string{
		"ghcr.io/you/app:v1",
		"you/app:v1",
		"app:v1",
		"registry.example.com:5000/team/app@sha256:" + "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef",
	}
	for _, r := range refs {
		if !IsReference(r) {
			t.Errorf("IsReference(%q) = false, want true", r)
		}
	}

	// Paths are never references — including a bare word, which would parse
	// as a Docker Hub image and turn every mistyped directory name into a
	// registry lookup.
	paths := []string{".", "./project", "/abs/path", "project", dir, filepath.Join(dir, "f")}
	for _, p := range paths {
		if IsReference(p) {
			t.Errorf("IsReference(%q) = true, want false", p)
		}
	}
}

// Credentials from the environment beat the Docker config, so CI can push
// with a token and no `docker login`.
func TestEnvCredentialsOverrideKeychain(t *testing.T) {
	t.Setenv(UsernameEnv, "u")
	t.Setenv(PasswordEnv, "p")
	if auth() == nil {
		t.Fatal("auth() returned nil with credentials set")
	}
	t.Setenv(UsernameEnv, "")
	t.Setenv(PasswordEnv, "")
	t.Setenv(TokenEnv, "tok")
	if auth() == nil {
		t.Fatal("auth() returned nil with a token set")
	}
}
