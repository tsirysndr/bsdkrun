# ci-woodpecker-plugins

A Woodpecker pipeline whose `fetch` step is a real plugin — the
`plugins/download` container image, settings flattened to `PLUGIN_*` env
plus Woodpecker's `CI_*` identity — executed without any Docker daemon:
the runner pulls the image's rootfs host-side, mounts it read-only into
the VM, overlays it writable and chroot-executes the entrypoint with the
workspace bound at `/drone/src`.

```sh
bsdkrun ci run -w .
```

The `verify` step asserts the plugin's output landed in the shared
workspace and prints `woodpecker-plugins-example-ok`.
