# ci-circleci — a CircleCI config, run locally by `bsdkrun ci`

The smallest useful CircleCI pipeline: it checks the platform's
identity environment, checks the clone landed, and prints
`circleci-example-ok`. `bsdkrun ci` detects `.circleci/config.yml` automatically —
no flag needed (use `--platform circleci` if several configs coexist).

CI runs the repository's **HEAD commit**, so the example needs its
own git repository:

```sh
cp -r examples/ci-circleci /tmp/ci-circleci
cd /tmp/ci-circleci
git init -q && git add -A && git commit -qm init
bsdkrun ci run
```
