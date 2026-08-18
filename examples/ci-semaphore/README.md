# ci-semaphore — a Semaphore config, run locally by `bsdkrun ci`

The smallest useful Semaphore pipeline: one block, one job, running in the
agent's container image. It checks the platform's identity environment,
checks the clone landed, and prints `semaphore-example-ok`. `bsdkrun ci`
detects `.semaphore/semaphore.yml` automatically — no flag needed (use
`--platform semaphore` if several configs coexist).

CI runs the repository's **HEAD commit**, so the example needs its own git
repository:

```sh
cp -r examples/ci-semaphore /tmp/ci-semaphore
cd /tmp/ci-semaphore
git init -q && git add -A && git commit -qm init
bsdkrun ci run
```
