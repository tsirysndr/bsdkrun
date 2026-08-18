# ci-gitlab — a GitLab CI config, run locally by `bsdkrun ci`

The smallest useful GitLab CI pipeline: it checks the platform's
identity environment, checks the clone landed, and prints
`gitlab-example-ok`. `bsdkrun ci` detects `.gitlab-ci.yml` automatically —
no flag needed (use `--platform gitlab` if several configs coexist).

CI runs the repository's **HEAD commit**, so the example needs its
own git repository:

```sh
cp -r examples/ci-gitlab /tmp/ci-gitlab
cd /tmp/ci-gitlab
git init -q && git add -A && git commit -qm init
bsdkrun ci run
```
