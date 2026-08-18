# ci-codebuild — a AWS CodeBuild config, run locally by `bsdkrun ci`

The smallest useful AWS CodeBuild pipeline: it checks the identity environment,
checks the clone landed, and prints `codebuild-example-ok`. `bsdkrun ci`
detects `buildspec.yml` automatically — no flag needed (use `--platform codebuild`
if several configs coexist).

CI runs the repository's **HEAD commit**, so the example needs its own git
repository:

```sh
cp -r examples/ci-codebuild /tmp/ci-codebuild
cd /tmp/ci-codebuild
git init -q && git add -A && git commit -qm init
bsdkrun ci run
```
