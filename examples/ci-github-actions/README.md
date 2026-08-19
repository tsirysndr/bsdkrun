# ci-github-actions — real `uses:` actions, run locally by `bsdkrun ci`

This workflow runs **the actual `oven-sh/setup-bun@v2` action** — not a
translation of it. `bsdkrun ci` fetches the action's `action.yml` to learn
what it is, clones it into the guest at its ref, provisions a node runtime,
and executes it under the genuine Actions protocol: `INPUT_*` from `with:`,
and `GITHUB_ENV`/`GITHUB_PATH`/`GITHUB_OUTPUT` command files whose effects
persist into every later step — which is why the plain `run:` step's
`bun --version` works: setup-bun wrote bun's location to `GITHUB_PATH`.

JavaScript and composite actions run for real. Container actions are
refused visibly (a microVM runs no Docker daemon), as are `pre`/`post`
hooks — stated limits, not silent ones.

CI runs the repository's **HEAD commit**, so the example needs its own git
repository:

```sh
cp -r examples/ci-github-actions /tmp/ci-github-actions
cd /tmp/ci-github-actions
git init -q && git add -A && git commit -qm init
bsdkrun ci run
```
