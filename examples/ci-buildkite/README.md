# ci-buildkite — a Buildkite config, run locally by `bsdkrun ci`

The smallest useful Buildkite pipeline: it checks the platform's
identity environment, checks the clone landed, and prints
`buildkite-example-ok`. `bsdkrun ci` detects `.buildkite/pipeline.yml` automatically —
no flag needed (use `--platform buildkite` if several configs coexist).

CI runs the repository's **HEAD commit**, so the example needs its
own git repository:

```sh
cp -r examples/ci-buildkite /tmp/ci-buildkite
cd /tmp/ci-buildkite
git init -q && git add -A && git commit -qm init
bsdkrun ci run
```
