# ci-drone-plugins — real Drone plugins, without a Docker daemon

A Drone plugin is a container image speaking the simplest protocol there
is: settings arrive as `PLUGIN_*` environment variables and the entrypoint
does the work. `bsdkrun ci` runs them for real — no daemon involved: the
plugin image's rootfs is pulled host-side at plan time (through the same
cache and registry resilience every image gets), mounted read-only into
the guest, given a writable overlay, the workspace bound at `/drone/src`,
and the entrypoint chroot-executed with the flattened settings — no shell
required inside the image, so scratch-plus-one-binary plugins work too.

This example runs the real [`plugins/download`](https://plugins.drone.io/plugins/download)
plugin to fetch a file into the workspace; the next step asserts the file
actually arrived — proof the plugin executed and shared the workspace.

Plugins that talk to a Docker daemon (`plugins/docker` building images)
still fail inside with their own error — the same honest boundary as
container actions.

CI runs the repository's **HEAD commit**, so the example needs its own git
repository:

```sh
cp -r examples/ci-drone-plugins /tmp/ci-drone-plugins
cd /tmp/ci-drone-plugins
git init -q && git add -A && git commit -qm init
bsdkrun ci run
```
