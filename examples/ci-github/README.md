# ci-github — a GitHub Actions config, run locally by `bsdkrun ci`

The smallest useful GitHub Actions pipeline: it checks the platform's
identity environment, checks the clone landed, and prints
`github-example-ok`. `bsdkrun ci` detects `.github/workflows/ci.yml` automatically —
no flag needed (use `--platform github` if several configs coexist).

CI runs the repository's **HEAD commit**, so the example needs its
own git repository:

```sh
cp -r examples/ci-github /tmp/ci-github
cd /tmp/ci-github
git init -q && git add -A && git commit -qm init
bsdkrun ci run
```
