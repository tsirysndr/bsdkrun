# ci-circleci-orbs

A CircleCI config that imports a real orb — `circleci/shellcheck@3` — and
references its `check` job from the workflow. The runner fetches the orb's
source from CircleCI's registry at plan time (the registry resolves the
partial `@3` to the newest 3.x itself) and expands it: the job's
parameterized docker executor becomes the VM image, `<< parameters.dir >>`
is substituted into the command, and `when`/`unless` branches are decided
from the resolved parameter values. Cache and artifact steps become
visible no-ops — a local run has no cross-run cache.

```sh
bsdkrun ci run -w .
```

The `done` job runs after the orb job and prints
`circleci-orbs-example-ok`.
