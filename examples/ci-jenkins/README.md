# ci-jenkins — a declarative Jenkinsfile, run locally by `bsdkrun ci`

The smallest useful declarative pipeline: a docker agent, an environment
block, one stage. It checks the identity environment, checks the clone
landed, and prints `jenkins-example-ok`. `bsdkrun ci` detects the
`Jenkinsfile` automatically — no flag needed (use `--platform jenkins` if
several configs coexist).

Only the **declarative** dialect translates: a scripted pipeline
(`node { ... }`) is an arbitrary Groovy program that nothing short of
Jenkins itself can run, and `bsdkrun ci` refuses it with a clear error
rather than mistranslating it.

CI runs the repository's **HEAD commit**, so the example needs its own git
repository:

```sh
cp -r examples/ci-jenkins /tmp/ci-jenkins
cd /tmp/ci-jenkins
git init -q && git add -A && git commit -qm init
bsdkrun ci run
```
