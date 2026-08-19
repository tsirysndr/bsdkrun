# ci-jenkins-real — a scripted Jenkinsfile, run by a real Jenkins

Scripted pipelines are Groovy programs; nothing short of Jenkins can run
one. So `bsdkrun ci` runs Jenkins: it assembles [Jenkinsfile
Runner](https://github.com/jenkinsci/jenkinsfile-runner) — the project's
official headless one-shot distribution — inside the guest (multi-arch JDK
image, the runner launcher, a pinned `jenkins.war`, plugins resolved by
the official plugin-installation-manager) and executes the Jenkinsfile in
an actual Jenkins with the CPS interpreter and real plugins.

Add a `plugins.txt` next to the Jenkinsfile to bring any additional
plugins; it is appended to the pipeline baseline and resolved with full
dependency handling.

The same road engages for declarative pipelines whose steps go beyond
`sh`/`echo`/`checkout` (junit, archiveArtifacts, `script { }` blocks…).
When everything translates structurally, the fast path stays — booting
Jenkins to run three shell steps would be ceremony, not fidelity.

CI runs the repository's **HEAD commit**, so the example needs its own git
repository:

```sh
cp -r examples/ci-jenkins-real /tmp/ci-jenkins-real
cd /tmp/ci-jenkins-real
git init -q && git add -A && git commit -qm init
bsdkrun ci run
```
