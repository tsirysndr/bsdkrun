# ci-tekton-catalog

A Tekton pipeline whose first task exists nowhere in this repository: it is
a `taskRef` through the hub resolver, so the runner fetches the real
`curl` task from the tektoncd catalog at plan time and expands it —
array params (`$(params.options[*])`) included, which become argv
elements rather than a substituted string.

The second task shows the other half: Tekton gives every step its own
container, so a step whose image differs from the one the VM booted is
run chrooted into its own pulled rootfs, sharing the workspace.

```sh
bsdkrun ci run -w .
```

The `report` step prints `tekton-catalog-example-ok` from a different
image than the task's first step.
