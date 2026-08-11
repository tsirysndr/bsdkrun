// Package oci pushes packed unikernels to an OCI registry and pulls them
// back.
//
// A unikernel is not a container image and cannot be run as one: it is a
// single bootable kernel with the application already linked in. What a
// registry gives it is distribution — the same authentication, mirroring,
// retention and access control an organisation already runs for its
// container images, rather than a second system for a second kind of
// artifact.
//
// The image is therefore an ordinary OCI artifact with unikernel-specific
// media types. A runtime that does not know them will refuse it rather than
// try to run it as a container, which is the correct outcome.
package oci

import (
	"archive/tar"
	"bytes"
	"compress/gzip"
	"encoding/json"
	"fmt"
	"io"
	"os"
	"path/filepath"
	"strings"

	"github.com/google/go-containerregistry/pkg/authn"
	"github.com/google/go-containerregistry/pkg/name"
	v1 "github.com/google/go-containerregistry/pkg/v1"
	"github.com/google/go-containerregistry/pkg/v1/empty"
	"github.com/google/go-containerregistry/pkg/v1/mutate"
	"github.com/google/go-containerregistry/pkg/v1/remote"
	"github.com/google/go-containerregistry/pkg/v1/tarball"
	"github.com/google/go-containerregistry/pkg/v1/types"
)

const (
	// MediaTypeConfig marks the descriptor holding Metadata below.
	MediaTypeConfig = "application/vnd.bsdkrun.unikernel.config.v1+json"
	// MediaTypeKernel marks the layer holding the kernel image.
	MediaTypeKernel = "application/vnd.bsdkrun.unikernel.kernel.v1.tar+gzip"

	// AnnotationCmdline carries the guest argv. Without it a puller has a
	// bootable kernel and no idea what to tell it to run — every provider
	// here has a different entrypoint, and the kernel does not record its
	// own.
	AnnotationCmdline = "dev.bsdkrun.unikernel.cmdline"
	AnnotationName    = "dev.bsdkrun.unikernel.name"
	AnnotationArch    = "dev.bsdkrun.unikernel.arch"
	// KernelFileName is what the kernel is called inside the layer, and on
	// disk after a pull.
	KernelFileName = "kernel"
)

// Credentials for a private registry. These override the Docker config,
// for CI and anywhere else that has a token but no `docker login`.
const (
	UsernameEnv = "BSDKRUN_REGISTRY_USERNAME"
	PasswordEnv = "BSDKRUN_REGISTRY_PASSWORD"
	TokenEnv    = "BSDKRUN_REGISTRY_TOKEN"
)

// InsecureEnv allows plain HTTP to a registry that is not localhost, for a
// private registry without TLS. Localhost needs no opt-in.
const InsecureEnv = "BSDKRUN_INSECURE_REGISTRY"

// parseRef resolves a reference.
//
// A reference with no registry defaults to Docker Hub, as everywhere else
// in the container ecosystem: "you/app:v1" is docker.io/you/app:v1, and a
// bare "app:v1" is docker.io/library/app:v1.
//
// It also allows plain HTTP where TLS cannot be
// expected. A registry on localhost is almost always a `registry:2`
// container with no certificate, and requiring one there would mean nobody
// could try this without standing up a CA.
func parseRef(ref string) (name.Reference, error) {
	if isLocal(ref) || os.Getenv(InsecureEnv) != "" {
		return name.ParseReference(ref, name.Insecure)
	}
	return name.ParseReference(ref)
}

func isLocal(ref string) bool {
	host, _, _ := strings.Cut(ref, "/")
	host, _, _ = strings.Cut(host, ":")
	return host == "localhost" || host == "127.0.0.1" || host == "::1"
}

// auth resolves credentials for a registry.
//
// The default is the Docker config — ~/.docker/config.json and whatever
// credential helper it names — so a registry the user has already run
// `docker login` against needs no further setup, and no credentials are
// duplicated into a second file. The environment overrides it for CI, where
// there is a token and no login.
func auth() remote.Option {
	if user, pass := os.Getenv(UsernameEnv), os.Getenv(PasswordEnv); user != "" && pass != "" {
		return remote.WithAuth(&authn.Basic{Username: user, Password: pass})
	}
	if token := os.Getenv(TokenEnv); token != "" {
		return remote.WithAuth(&authn.Bearer{Token: token})
	}
	return remote.WithAuthFromKeychain(authn.DefaultKeychain)
}

// Metadata is the image config: everything needed to boot the kernel that
// the kernel itself does not carry.
type Metadata struct {
	Name     string `json:"name"`
	Provider string `json:"provider"`
	Arch     string `json:"arch"`
	Cmdline  string `json:"cmdline"`
	Kernel   string `json:"kernel"`
}

// Push uploads kernelPath to ref, with meta as the image config.
func Push(ref, kernelPath string, meta Metadata, onProgress func(string)) (string, error) {
	tag, err := parseRef(ref)
	if err != nil {
		return "", fmt.Errorf("parsing %q: %w", ref, err)
	}

	layer, err := kernelLayer(kernelPath)
	if err != nil {
		return "", err
	}

	config, err := json.Marshal(meta)
	if err != nil {
		return "", err
	}

	img := mutate.MediaType(empty.Image, types.OCIManifestSchema1)
	img = mutate.ConfigMediaType(img, MediaTypeConfig)
	img, err = mutate.Append(img, mutate.Addendum{Layer: layer, MediaType: MediaTypeKernel})
	if err != nil {
		return "", err
	}
	img, err = mutate.Config(img, v1.Config{})
	if err != nil {
		return "", err
	}
	img = mutate.Annotations(img, map[string]string{
		AnnotationName:    meta.Name,
		AnnotationArch:    meta.Arch,
		AnnotationCmdline: meta.Cmdline,
	}).(v1.Image)

	// The raw config blob is what carries Metadata; go-containerregistry
	// builds a container config by default, which a unikernel has no use
	// for.
	img = &configImage{Image: img, raw: config}

	if onProgress != nil {
		onProgress(fmt.Sprintf("pushing %s", tag.Name()))
	}
	if err := remote.Write(tag, img, auth()); err != nil {
		return "", fmt.Errorf("pushing %s: %w", tag.Name(), err)
	}

	digest, err := img.Digest()
	if err != nil {
		return "", err
	}
	return tag.Context().Name() + "@" + digest.String(), nil
}

// Pull downloads ref into destDir, writing the kernel and a metadata file
// beside it, and returns the metadata.
//
// The pull is atomic: it lands in a temporary directory and is renamed into
// place, so an interrupted pull cannot leave a half-written kernel that
// looks cached. A boot reaching for a truncated kernel fails in the guest,
// where there is nothing to explain it.
func Pull(ref, destDir string, onProgress func(string)) (*Metadata, error) {
	tag, err := parseRef(ref)
	if err != nil {
		return nil, fmt.Errorf("parsing %q: %w", ref, err)
	}

	if onProgress != nil {
		onProgress(fmt.Sprintf("pulling %s", tag.Name()))
	}
	img, err := remote.Image(tag, auth())
	if err != nil {
		return nil, fmt.Errorf("fetching %s: %w", tag.Name(), err)
	}

	manifest, err := img.Manifest()
	if err != nil {
		return nil, err
	}
	if manifest.Config.MediaType != MediaTypeConfig {
		return nil, fmt.Errorf("%s is not a bsdkrun unikernel (config media type %s)",
			tag.Name(), manifest.Config.MediaType)
	}

	rawConfig, err := img.RawConfigFile()
	if err != nil {
		return nil, err
	}
	var meta Metadata
	if err := json.Unmarshal(rawConfig, &meta); err != nil {
		return nil, fmt.Errorf("reading unikernel metadata: %w", err)
	}

	layers, err := img.Layers()
	if err != nil {
		return nil, err
	}
	if len(layers) != 1 {
		return nil, fmt.Errorf("expected one layer, got %d", len(layers))
	}

	if err := os.MkdirAll(filepath.Dir(destDir), 0o755); err != nil {
		return nil, err
	}
	staging, err := os.MkdirTemp(filepath.Dir(destDir), ".pull-*")
	if err != nil {
		return nil, err
	}
	defer os.RemoveAll(staging)

	rc, err := layers[0].Uncompressed()
	if err != nil {
		return nil, err
	}
	defer rc.Close()

	if err := extractKernel(rc, filepath.Join(staging, KernelFileName)); err != nil {
		return nil, err
	}
	if err := os.WriteFile(filepath.Join(staging, "metadata.json"), rawConfig, 0o644); err != nil {
		return nil, err
	}

	os.RemoveAll(destDir)
	if err := os.Rename(staging, destDir); err != nil {
		return nil, err
	}
	return &meta, nil
}

// extractKernel pulls the single kernel entry out of the layer tar.
func extractKernel(r io.Reader, dest string) error {
	tr := tar.NewReader(r)
	for {
		header, err := tr.Next()
		if err == io.EOF {
			return fmt.Errorf("layer holds no %s", KernelFileName)
		}
		if err != nil {
			return err
		}
		if header.Typeflag != tar.TypeReg || filepath.Base(header.Name) != KernelFileName {
			continue
		}
		f, err := os.OpenFile(dest, os.O_CREATE|os.O_WRONLY|os.O_TRUNC, 0o755)
		if err != nil {
			return err
		}
		defer f.Close()
		// io.Copy rather than a bounded copy: the size is the registry's
		// own, already verified against the layer digest by the time the
		// reader hands over a byte.
		if _, err := io.Copy(f, tr); err != nil {
			return err
		}
		return nil
	}
}

// kernelLayer wraps the kernel in a one-entry gzipped tar. A tar for a
// single file is redundant, but it is what every registry, mirror and
// scanner expects a layer to be, and the alternative is an artifact only
// this tool can read.
func kernelLayer(kernelPath string) (v1.Layer, error) {
	body, err := os.ReadFile(kernelPath)
	if err != nil {
		return nil, fmt.Errorf("reading kernel: %w", err)
	}

	var buf bytes.Buffer
	zw := gzip.NewWriter(&buf)
	tw := tar.NewWriter(zw)
	if err := tw.WriteHeader(&tar.Header{
		Name:     KernelFileName,
		Mode:     0o755,
		Size:     int64(len(body)),
		Typeflag: tar.TypeReg,
	}); err != nil {
		return nil, err
	}
	if _, err := tw.Write(body); err != nil {
		return nil, err
	}
	if err := tw.Close(); err != nil {
		return nil, err
	}
	if err := zw.Close(); err != nil {
		return nil, err
	}

	return tarball.LayerFromOpener(
		func() (io.ReadCloser, error) {
			return io.NopCloser(bytes.NewReader(buf.Bytes())), nil
		},
		tarball.WithMediaType(MediaTypeKernel),
	)
}

// configImage substitutes a raw config blob for the container config
// go-containerregistry would otherwise generate. A unikernel's config is
// Metadata, not a set of container runtime defaults.
type configImage struct {
	v1.Image
	raw []byte
}

func (c *configImage) RawConfigFile() ([]byte, error) { return c.raw, nil }

func (c *configImage) Digest() (v1.Hash, error) {
	// Appending to the manifest changes the digest, so it is recomputed
	// from the manifest this type actually serves rather than the embedded
	// image's.
	manifest, err := c.RawManifest()
	if err != nil {
		return v1.Hash{}, err
	}
	hash, _, err := v1.SHA256(bytes.NewReader(manifest))
	return hash, err
}

func (c *configImage) RawManifest() ([]byte, error) {
	manifest, err := c.Image.Manifest()
	if err != nil {
		return nil, err
	}
	// Point the config descriptor at the raw blob: size and digest have to
	// describe what is actually uploaded, or the registry rejects the
	// manifest.
	hash, size, err := v1.SHA256(bytes.NewReader(c.raw))
	if err != nil {
		return nil, err
	}
	updated := *manifest
	updated.Config = v1.Descriptor{
		MediaType: MediaTypeConfig,
		Size:      size,
		Digest:    hash,
	}
	return json.Marshal(updated)
}

// IsReference reports whether s looks like a registry reference rather than
// a local path. A path that exists is always a path; the question only
// arises for one that does not.
func IsReference(s string) bool {
	if _, err := os.Stat(s); err == nil {
		return false
	}
	if strings.HasPrefix(s, ".") || strings.HasPrefix(s, "/") {
		return false
	}
	if _, err := name.ParseReference(s); err != nil {
		return false
	}
	// A bare word parses as a Docker Hub reference ("ubuntu" ->
	// docker.io/library/ubuntu), which would make every mistyped directory
	// name a registry lookup. Require something that says "registry":
	// a registry host, a namespace, a tag or a digest.
	return strings.ContainsAny(s, "/:@")
}
