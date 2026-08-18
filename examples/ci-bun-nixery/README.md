# ci-bun-nixery — bun tests with the toolchain from nixery

The same tests as [`ci-bun-ubuntu`](../ci-bun-ubuntu), but the workflow lists
`bun` under `dependencies:` instead of naming an image. The runner maps the
dependency list to a nixery image (bun plus the default toolchain), so the
guest boots with bun already on PATH and the workflow is a single step.

The first run can wait a while: nixery builds the image server-side on its
first request. It is cached from then on. If nixery times out, the runner
falls back to the pinned `nixos/nix` image and installs the same dependencies
with `nix profile add` — announced in the log, and slower, but it finishes.

CI runs the repository's **HEAD commit**, so the example must live in its own
git repository with the files committed:

```sh
cp -r examples/ci-bun-nixery /tmp/ci-bun-nixery
cd /tmp/ci-bun-nixery
git init -q && git add -A && git commit -qm init
bsdkrun ci run test
```
