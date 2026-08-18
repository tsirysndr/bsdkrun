# ci-woodpecker — a Woodpecker config, run locally by `bsdkrun ci`

The smallest useful Woodpecker pipeline: it checks the platform's
identity environment, checks the clone landed, and prints
`woodpecker-example-ok`. `bsdkrun ci` detects `.woodpecker/test.yml` automatically —
no flag needed (use `--platform woodpecker` if several configs coexist).

CI runs the repository's **HEAD commit**, so the example needs its
own git repository:

```sh
cp -r examples/ci-woodpecker /tmp/ci-woodpecker
cd /tmp/ci-woodpecker
git init -q && git add -A && git commit -qm init
bsdkrun ci run
```
