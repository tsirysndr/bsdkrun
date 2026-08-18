# ci-tekton — a Tekton config, run locally by `bsdkrun ci`

The smallest useful Tekton pipeline: it checks the clone landed (through a substituted Tekton param),
checks the clone landed, and prints `tekton-example-ok`. `bsdkrun ci`
detects `.tekton/*.yaml` automatically — no flag needed (use `--platform tekton`
if several configs coexist).

CI runs the repository's **HEAD commit**, so the example needs its own git
repository:

```sh
cp -r examples/ci-tekton /tmp/ci-tekton
cd /tmp/ci-tekton
git init -q && git add -A && git commit -qm init
bsdkrun ci run
```
