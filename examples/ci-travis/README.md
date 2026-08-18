# ci-travis — a Travis CI config, run locally by `bsdkrun ci`

The smallest useful Travis CI pipeline: it checks the platform's
identity environment, checks the clone landed, and prints
`travis-example-ok`. `bsdkrun ci` detects `.travis.yml` automatically —
no flag needed (use `--platform travis` if several configs coexist).

CI runs the repository's **HEAD commit**, so the example needs its
own git repository:

```sh
cp -r examples/ci-travis /tmp/ci-travis
cd /tmp/ci-travis
git init -q && git add -A && git commit -qm init
bsdkrun ci run
```
