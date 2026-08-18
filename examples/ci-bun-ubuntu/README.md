# ci-bun-ubuntu — bun tests on an Ubuntu microVM

A minimal tangled workflow that runs `bun test` inside a plain
`ubuntu:24.04` microVM — no nixery. The workflow's `image:` is an ordinary
OCI reference, so the runner boots it directly and installs nothing except
git (needed to clone your commit into the guest). Bun itself is installed by
the workflow's own first step, the way it would be on any stock Ubuntu box.

CI runs the repository's **HEAD commit**, so the example must live in its own
git repository with the files committed:

```sh
cp -r examples/ci-bun-ubuntu /tmp/ci-bun-ubuntu
cd /tmp/ci-bun-ubuntu
git init -q && git add -A && git commit -qm init
bsdkrun ci run test
```

Compare with [`ci-bun-nixery`](../ci-bun-nixery), which runs the same tests
with bun coming from a nixery image instead — no install step at all.
