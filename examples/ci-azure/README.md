# ci-azure — a Azure Pipelines config, run locally by `bsdkrun ci`

The smallest useful Azure Pipelines pipeline: it checks the identity environment,
checks the clone landed, and prints `azure-example-ok`. `bsdkrun ci`
detects `azure-pipelines.yml` automatically — no flag needed (use `--platform azure`
if several configs coexist).

CI runs the repository's **HEAD commit**, so the example needs its own git
repository:

```sh
cp -r examples/ci-azure /tmp/ci-azure
cd /tmp/ci-azure
git init -q && git add -A && git commit -qm init
bsdkrun ci run
```
