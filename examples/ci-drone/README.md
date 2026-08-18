# ci-drone — a Drone config, run locally by `bsdkrun ci`

The smallest useful Drone pipeline: it checks the platform's
identity environment, checks the clone landed, and prints
`drone-example-ok`. `bsdkrun ci` detects `.drone.yml` automatically —
no flag needed (use `--platform drone` if several configs coexist).

CI runs the repository's **HEAD commit**, so the example needs its
own git repository:

```sh
cp -r examples/ci-drone /tmp/ci-drone
cd /tmp/ci-drone
git init -q && git add -A && git commit -qm init
bsdkrun ci run
```
