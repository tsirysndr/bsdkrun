import { useEffect, useState } from "react";
import {
  Button,
  Input,
  Modal,
  ModalBody,
  ModalContent,
  ModalFooter,
  ModalHeader,
  Switch,
  Tab,
  Tabs,
  Textarea,
} from "@heroui/react";
import { useAtom, useSetAtom } from "jotai";
import { Controller, useForm } from "react-hook-form";
import { zodResolver } from "@hookform/resolvers/zod";
import { z } from "zod";
import { IconRocket, IconChevronDown } from "@tabler/icons-react";
import { runOpenAtom, runPrefillAtom, viewAtom } from "../state/atoms";
import { useNetworks, useVersions } from "../lib/queries";
import { useLaunchMachine } from "../hooks/useLaunchFlavor";
import type { RunSpec } from "../lib/types";

type Kind = "linux" | "freebsd" | "netbsd" | "unikraft" | "nanos" | "osv" | "solo5";

const KIND_LABEL: Record<Kind, string> = {
  linux: "Linux (OCI)",
  freebsd: "FreeBSD",
  netbsd: "NetBSD",
  unikraft: "Unikraft",
  nanos: "Nanos",
  osv: "OSv",
  solo5: "Solo5 (MirageOS)",
};

// ---- zod schema ------------------------------------------------------------

const schema = z
  .object({
    kind: z.enum(["linux", "freebsd", "netbsd", "unikraft", "nanos", "osv", "solo5"]),
    image: z.string(),
    path: z.string(),
    cmdline: z.string(),
    guestArgs: z.string(),
    blocks: z
      .string()
      .refine(
        (s) =>
          s
            .split("\n")
            .map((l) => l.trim())
            .filter(Boolean)
            .every((l) => /^([^=\s]+=)?\S+$/.test(l)),
        "Each line must be NAME=FILE (NAME= optional with a single device)",
      ),
    version: z.string(),
    cpus: z.coerce
      .number({ invalid_type_error: "Enter a number" })
      .int()
      .min(1, "At least 1")
      .max(64, "Too many"),
    mem: z.coerce
      .number({ invalid_type_error: "Enter a number" })
      .int()
      .min(128, "Min 128 MiB"),
    volume: z
      .string()
      .regex(/^[a-zA-Z0-9._-]*$/, "Letters, digits, . _ - only"),
    command: z.string(),
    repo: z.string(),
    network: z.string(),
    machineName: z
      .string()
      .regex(/^[a-zA-Z0-9._-]*$/, "Letters, digits, . _ - only"),
    noNet: z.boolean(),
    initramfs: z.boolean(),
    entrypoint: z.string(),
    mounts: z.string(),
    ports: z
      .string()
      .refine(
        (s) =>
          s
            .split("\n")
            .map((l) => l.trim())
            .filter(Boolean)
            .every((l) => /^((\d{1,3}\.){3}\d{1,3}:)?\d+:\d+$/.test(l)),
        "Each line must be HOST:GUEST or BIND:HOST:GUEST (e.g. 0.0.0.0:8080:80)",
      ),
    attachDisks: z.string(),
    diskSize: z
      .string()
      .regex(/^$|^\d+(\.\d+)?\s*[KMGT]?B?$/i, "e.g. 8G, 4096M"),
  })
  .refine((d) => d.kind !== "linux" || d.image.trim().length > 0, {
    message: "An image reference is required",
    path: ["image"],
  })
  .refine((d) => d.kind !== "unikraft" || d.path.trim().length > 0, {
    message: "A kraft project directory or unikernel image is required",
    path: ["path"],
  })
  .refine((d) => d.kind !== "nanos" || d.image.trim().length > 0, {
    message: "An ops image (path or ~/.ops/images name) is required",
    path: ["image"],
  })
  .refine((d) => d.kind !== "osv" || d.image.trim().length > 0, {
    message: "An OSv image (loader.img, or a capstan-composed image) is required",
    path: ["image"],
  })
  .refine((d) => d.kind !== "solo5" || d.path.trim().length > 0, {
    message: "A .hvt unikernel or a mirage project directory is required",
    path: ["path"],
  });

type FormValues = z.infer<typeof schema>;

const DEFAULTS: FormValues = {
  kind: "linux",
  image: "alpine",
  path: "",
  cmdline: "",
  guestArgs: "",
  blocks: "",
  version: "",
  cpus: 1,
  mem: 512,
  volume: "",
  command: "",
  repo: "",
  network: "",
  machineName: "",
  noNet: false,
  initramfs: false,
  entrypoint: "",
  mounts: "",
  ports: "",
  attachDisks: "",
  diskSize: "",
};

const splitArgs = (s: string) => (s.trim() ? s.trim().split(/\s+/) : []);
const lines = (s: string) =>
  s
    .split("\n")
    .map((l) => l.trim())
    .filter(Boolean);

export default function RunDialog() {
  const [open, setOpen] = useAtom(runOpenAtom);
  const [prefill, setPrefill] = useAtom(runPrefillAtom);
  const setView = useSetAtom(viewAtom);
  const launchMachine = useLaunchMachine();
  const [advanced, setAdvanced] = useState(false);

  const {
    control,
    handleSubmit,
    watch,
    setValue,
    reset,
    formState: { errors, isSubmitting },
  } = useForm<FormValues>({
    resolver: zodResolver(schema),
    defaultValues: DEFAULTS,
    mode: "onChange",
  });

  const kind = watch("kind") as Kind;
  const bsd = kind === "freebsd" || kind === "netbsd";
  // A unikernel is the application linked into the kernel: no disk, no
  // userland. Everything disk- or agent-shaped is hidden for it.
  const unikraft = kind === "unikraft";
  const nanosKind = kind === "nanos";
  const osvKind = kind === "osv";
  // Solo5 runs under its own tender, not libkrun, but is one more agent-less
  // unikernel as far as the dialog is concerned.
  const solo5 = kind === "solo5";
  // Shared "no agent, no repo/command" handling for every unikernel kind.
  const unikernel = unikraft || nanosKind || osvKind || solo5;
  const version = watch("version");

  const { data: allVersions = [] } = useVersions(kind, open && bsd);
  const { data: networks = [] } = useNetworks();
  // Only NetBSD `current` (HEAD, ≈ NetBSD 11) has modern virtio-mmio that boots
  // to root under libkrun; the numbered 8–10 releases use legacy virtio and
  // can't mount their root disk, so don't offer them.
  const versions =
    kind === "netbsd"
      ? allVersions.filter((v) => v.version === "current")
      : allVersions;

  // Apply prefill when the dialog opens.
  useEffect(() => {
    if (open && prefill) {
      if (prefill.kind) setValue("kind", prefill.kind as Kind);
      if (prefill.image) setValue("image", prefill.image);
      setPrefill(null);
    }
  }, [open, prefill, setPrefill, setValue]);

  // Reset the picked version when the guest kind changes so the per-OS default
  // (below) re-applies instead of carrying a FreeBSD version over to NetBSD.
  useEffect(() => {
    setValue("version", "");
  }, [kind, setValue]);

  // Default the NetBSD version to `current` (the build that boots under libkrun),
  // else the release the CLI marks as latest. FreeBSD always uses the bundled
  // arm64 image (agent injected), so it needs no version.
  useEffect(() => {
    if (kind === "netbsd" && versions.length && !version) {
      const chosen =
        versions.find((v) => v.version === "current") ||
        versions.find((v) => v.latest) ||
        versions[versions.length - 1];
      if (chosen) setValue("version", chosen.version);
    }
  }, [versions, version, kind, setValue]);

  const onSubmit = async (data: FormValues) => {
    const spec: RunSpec = {
      kind: data.kind,
      image:
        data.kind === "linux" ||
        data.kind === "nanos" ||
        data.kind === "osv"
          ? data.image.trim()
          : null,
      path:
        data.kind === "unikraft" || data.kind === "solo5"
          ? data.path.trim()
          : null,
      // Solo5 takes guest `args`, not a kernel cmdline.
      cmdline:
        unikernel && !solo5 && data.cmdline.trim() ? data.cmdline.trim() : null,
      // Only NetBSD takes a --version (it selects the bundled kernel). A FreeBSD
      // --version makes the CLI DOWNLOAD the official (agent-less) image, which
      // the GUI can't exec/terminal into — always use the bundled arm64 image.
      version: data.kind === "netbsd" && data.version ? data.version : null,
      // The solo5-hvt tender is single-vCPU; the control is disabled, so pin
      // the value rather than send whatever the form last held.
      cpus: solo5 ? 1 : data.cpus,
      mem: data.mem,
      volume: unikernel ? null : data.volume.trim() || null,
      no_net: data.noNet,
      initramfs: data.kind === "linux" ? data.initramfs : false,
      entrypoint:
        data.kind === "linux" && data.entrypoint.trim()
          ? data.entrypoint.trim()
          : null,
      mounts:
        data.kind === "linux" || data.kind === "unikraft"
          ? lines(data.mounts)
          : [],
      ports: lines(data.ports),
      attach_disks:
        bsd || data.kind === "linux" ? lines(data.attachDisks) : [],
      disk_size: bsd && data.diskSize.trim() ? data.diskSize.trim() : null,
      repo: unikernel || !data.repo.trim() ? null : data.repo.trim(),
      network: data.network.trim() ? data.network.trim() : null,
      name: data.machineName.trim() ? data.machineName.trim() : null,
      command: unikernel ? [] : splitArgs(data.command),
      blocks: solo5 ? lines(data.blocks) : [],
      args: solo5 ? splitArgs(data.guestArgs) : [],
    };
    // Stream the launch (pull / download / boot) in the progress modal instead
    // of blocking the dialog on a silent spinner — close the dialog right away.
    const label =
      data.kind === "linux" ||
      data.kind === "nanos" ||
      data.kind === "osv"
        ? data.image.trim() || KIND_LABEL[data.kind]
        : data.kind === "unikraft" || data.kind === "solo5"
          ? data.path.trim().split("/").filter(Boolean).pop() || "unikernel"
          : KIND_LABEL[data.kind];
    launchMachine(label, spec);
    setOpen(false);
    reset(DEFAULTS);
    setAdvanced(false);
    setView("machines");
  };

  return (
    <Modal
      isOpen={open}
      onClose={() => setOpen(false)}
      size="2xl"
      backdrop="opaque"
      scrollBehavior="inside"
      shouldBlockScroll={false}
      classNames={{ base: "border border-white/10 bg-content1" }}
    >
      <ModalContent>
        <form onSubmit={handleSubmit(onSubmit)}>
          <ModalHeader className="flex flex-col gap-1">
            <div className="flex items-center gap-2 text-base">
              <IconRocket size={18} className="text-primary" />
              Run a new machine
            </div>
            <p className="text-xs font-normal text-foreground-500">
              Launches detached — like <span className="font-mono">docker run -d</span>.
            </p>
          </ModalHeader>
          <ModalBody className="gap-4">
            <Controller
              control={control}
              name="kind"
              render={({ field }) => (
                <Tabs
                  aria-label="Guest kind"
                  color="primary"
                  radius="lg"
                  selectedKey={field.value}
                  onSelectionChange={(k) => field.onChange(k)}
                  classNames={{ tabList: "bg-content2/60" }}
                >
                  <Tab key="linux" title="Linux (OCI)" />
                  <Tab key="freebsd" title="FreeBSD" />
                  <Tab key="netbsd" title="NetBSD" />
                  <Tab key="unikraft" title="Unikraft" />
                  <Tab key="nanos" title="Nanos" />
                  <Tab key="osv" title="OSv" />
                  <Tab key="solo5" title="Solo5" />
                </Tabs>
              )}
            />

            {kind === "linux" ? (
              <Controller
                control={control}
                name="image"
                render={({ field }) => (
                  <Input
                    label="Image reference"
                    labelPlacement="outside"
                    placeholder="alpine, ubuntu:24.04, ghcr.io/owner/name:tag"
                    value={field.value}
                    onValueChange={field.onChange}
                    isRequired
                    variant="bordered"
                    isInvalid={!!errors.image}
                    errorMessage={errors.image?.message}
                  />
                )}
              />
            ) : nanosKind ? (
              <>
                <Controller
                  control={control}
                  name="image"
                  render={({ field }) => (
                    <Input
                      label="Nanos image"
                      labelPlacement="outside"
                      placeholder="nanos-hello  (a path, or a name in ~/.ops/images)"
                      value={field.value}
                      onValueChange={field.onChange}
                      isRequired
                      variant="bordered"
                      isInvalid={!!errors.image}
                      errorMessage={errors.image?.message}
                      description="Build it first with `ops build <elf> -i <name>`. See examples/nanos-hello."
                      classNames={{ input: "font-mono text-xs" }}
                    />
                  )}
                />
                <div className="rounded-xl border border-white/10 bg-content2/40 px-3 py-2 text-xs text-foreground-400">
                  A Nanos unikernel — no shell or agent, so the terminal and
                  snapshots don&apos;t apply. Linux/x86_64 is experimental;
                  macOS/arm64 needs an upstream Nanos fix. Use logs to read its
                  output.
                </div>
              </>
            ) : osvKind ? (
              <>
                <Controller
                  control={control}
                  name="image"
                  render={({ field }) => (
                    <Input
                      label="OSv image"
                      labelPlacement="outside"
                      placeholder="disk.raw  (a loader.img, or a capstan-composed image)"
                      value={field.value}
                      onValueChange={field.onChange}
                      isRequired
                      variant="bordered"
                      isInvalid={!!errors.image}
                      errorMessage={errors.image?.message}
                      description="Compose one with `capstan package compose --fs rofs`. See examples/osv-hello."
                      classNames={{ input: "font-mono text-xs" }}
                    />
                  )}
                />
                <div className="rounded-xl border border-white/10 bg-content2/40 px-3 py-2 text-xs text-foreground-400">
                  An OSv unikernel — no shell or agent, so the terminal and
                  snapshots don&apos;t apply. The command line is the
                  application to run, e.g. <code>/hello.so</code>. Use logs to
                  read its output.
                </div>
              </>
            ) : solo5 ? (
              <>
                <Controller
                  control={control}
                  name="path"
                  render={({ field }) => (
                    <Input
                      label="Unikernel"
                      labelPlacement="outside"
                      placeholder="~/projects/hello  (a .hvt binary, or a mirage project dir)"
                      value={field.value}
                      onValueChange={field.onChange}
                      isRequired
                      variant="bordered"
                      isInvalid={!!errors.path}
                      errorMessage={errors.path?.message}
                      description="Build it first with `mirage configure -t hvt && mirage build`; a project directory is searched for the .hvt under dist/."
                      classNames={{ input: "font-mono text-xs" }}
                    />
                  )}
                />
                <div className="rounded-xl border border-white/10 bg-content2/40 px-3 py-2 text-xs text-foreground-400">
                  A MirageOS unikernel, run under the solo5-hvt tender — no
                  disk and no shell, so volumes, snapshots and the terminal
                  don&apos;t apply. Use logs to read its output.
                </div>
              </>
            ) : unikraft ? (
              <>
                <Controller
                  control={control}
                  name="path"
                  render={({ field }) => (
                    <Input
                      label="Unikernel"
                      labelPlacement="outside"
                      placeholder="~/projects/helloworld  (a kraft project dir, or a built image)"
                      value={field.value}
                      onValueChange={field.onChange}
                      isRequired
                      variant="bordered"
                      isInvalid={!!errors.path}
                      errorMessage={errors.path?.message}
                      description="Build it first with `kraft build --plat fc`; a project directory is searched for the image under .unikraft/build/."
                      classNames={{ input: "font-mono text-xs" }}
                    />
                  )}
                />
                <div className="rounded-xl border border-white/10 bg-content2/40 px-3 py-2 text-xs text-foreground-400">
                  The application is linked into the kernel — there is no disk and
                  no shell, so volumes, snapshots and the terminal don&apos;t apply.
                  Use logs to read its output.
                </div>
              </>
            ) : kind === "netbsd" ? (
              <Controller
                control={control}
                name="version"
                render={({ field }) => (
                  <div>
                    <label className="mb-1.5 block text-sm">Release</label>
                    <div className="relative">
                      {/* Native <select> on purpose: HeroUI's Select popover runs
                          react-aria usePreventScroll, which freezes Tauri's
                          WKWebView. A native control has no such overlay. */}
                      <select
                        value={field.value}
                        onChange={(e) => field.onChange(e.target.value)}
                        className="w-full appearance-none rounded-xl border border-white/10 bg-content2/60 px-3 py-2 pr-9 text-sm text-foreground outline-none transition [color-scheme:dark] hover:border-white/20 focus:border-white/30"
                      >
                        <option value="" disabled>
                          {versions.length ? "Select a version" : "Loading versions…"}
                        </option>
                        {versions.map((v) => (
                          <option key={v.version} value={v.version}>
                            {kind === "netbsd" && v.version === "current"
                              ? "current  (NetBSD 11 · virtio)"
                              : `${v.version}${v.latest ? "  (latest)" : ""}`}
                          </option>
                        ))}
                      </select>
                      <IconChevronDown
                        size={16}
                        className="pointer-events-none absolute right-3 top-1/2 -translate-y-1/2 text-foreground-400"
                      />
                    </div>
                  </div>
                )}
              />
            ) : (
              <div>
                <label className="mb-1.5 block text-sm">Release</label>
                <div className="rounded-xl border border-white/10 bg-content2/40 px-3 py-2 text-sm text-foreground-400">
                  Bundled FreeBSD image{" "}
                  <span className="text-foreground-500">
                    (agent injected — exec &amp; terminal work out of the box)
                  </span>
                </div>
              </div>
            )}

            {bsd && (
              <Controller
                control={control}
                name="diskSize"
                render={({ field }) => (
                  <Input
                    label="Disk size"
                    labelPlacement="outside"
                    placeholder="default (image size) — e.g. 8G, 4096M"
                    value={field.value}
                    onValueChange={field.onChange}
                    variant="bordered"
                    isInvalid={!!errors.diskSize}
                    errorMessage={errors.diskSize?.message}
                    description="Grows the root disk (only enlarges); the guest expands its filesystem on boot."
                    classNames={{ input: "font-mono text-xs" }}
                  />
                )}
              />
            )}

            <div className={unikernel ? "grid grid-cols-2 gap-3" : "grid grid-cols-3 gap-3"}>
              <Controller
                control={control}
                name="cpus"
                render={({ field }) => (
                  <Input
                    type="number"
                    label="vCPUs"
                    labelPlacement="outside"
                    // The solo5-hvt tender is single-vCPU: disable the control
                    // and show the 1 that will be used, rather than accept a
                    // number the engine would warn about and ignore.
                    value={solo5 ? "1" : String(field.value)}
                    onValueChange={field.onChange}
                    min={1}
                    variant="bordered"
                    isDisabled={solo5}
                    description={solo5 ? "Solo5 is single-vCPU" : undefined}
                    isInvalid={!!errors.cpus}
                    errorMessage={errors.cpus?.message}
                  />
                )}
              />
              <Controller
                control={control}
                name="mem"
                render={({ field }) => (
                  <Input
                    type="number"
                    label="Memory (MiB)"
                    labelPlacement="outside"
                    value={String(field.value)}
                    onValueChange={field.onChange}
                    min={128}
                    step={128}
                    variant="bordered"
                    isInvalid={!!errors.mem}
                    errorMessage={errors.mem?.message}
                  />
                )}
              />
              {/* No volume for a unikernel: there is no disk to persist. */}
              {!unikernel && (
                <Controller
                  control={control}
                  name="volume"
                  render={({ field }) => (
                    <Input
                      label="Volume"
                      labelPlacement="outside"
                      placeholder="optional name"
                      value={field.value}
                      onValueChange={field.onChange}
                      variant="bordered"
                      isInvalid={!!errors.volume}
                      errorMessage={errors.volume?.message}
                    />
                  )}
                />
              )}
            </div>

            {/* A unikernel takes a kernel cmdline (which Unikraft hands the
                application as argv) rather than an agent-run command. Solo5
                instead takes guest arguments, passed after a literal `--`
                because MirageOS options look like bsdkrun's own flags. */}
            {solo5 ? (
              <Controller
                control={control}
                name="guestArgs"
                render={({ field }) => (
                  <Input
                    label="Guest arguments"
                    labelPlacement="outside"
                    placeholder="--ipv4=10.0.0.2/24 --port 8080"
                    value={field.value}
                    onValueChange={field.onChange}
                    variant="bordered"
                    description="Passed to the unikernel itself (after `--` on the CLI)."
                    classNames={{ input: "font-mono text-xs" }}
                  />
                )}
              />
            ) : unikernel ? (
              <Controller
                control={control}
                name="cmdline"
                render={({ field }) => (
                  <Input
                    label="Command line"
                    labelPlacement="outside"
                    placeholder="helloworld --port 8080"
                    value={field.value}
                    onValueChange={field.onChange}
                    variant="bordered"
                    description="Passed to the application as argv — the first word is argv[0]."
                    classNames={{ input: "font-mono text-xs" }}
                  />
                )}
              />
            ) : (
              <>
                <Controller
                  control={control}
                  name="command"
                  render={({ field }) => (
                    <Input
                      label="Command"
                      labelPlacement="outside"
                      placeholder={
                        kind === "linux"
                          ? "override CMD, e.g. sh -c 'echo hi'"
                          : "run once via agent, e.g. uname -a (optional)"
                      }
                      value={field.value}
                      onValueChange={field.onChange}
                      variant="bordered"
                      classNames={{ input: "font-mono text-xs" }}
                    />
                  )}
                />

                <Controller
                  control={control}
                  name="repo"
                  render={({ field }) => (
                    <Input
                      label="Git repository"
                      labelPlacement="outside"
                      placeholder="https://github.com/owner/name (cloned & cd'd on shell)"
                      value={field.value}
                      onValueChange={field.onChange}
                      variant="bordered"
                      classNames={{ input: "font-mono text-xs" }}
                    />
                  )}
                />
              </>
            )}

            <div className="grid grid-cols-2 gap-3">
              <Controller
                control={control}
                name="network"
                render={({ field }) => (
                  <div>
                    <label className="mb-1.5 block text-sm">Network</label>
                    <div className="relative">
                      {/* Native select — HeroUI's Select popover freezes WKWebView. */}
                      <select
                        value={field.value}
                        onChange={(e) => field.onChange(e.target.value)}
                        className="w-full appearance-none rounded-xl border border-white/10 bg-content2/60 px-3 py-2 pr-9 text-sm text-foreground outline-none transition [color-scheme:dark] hover:border-white/20 focus:border-white/30"
                      >
                        <option value="">None (isolated)</option>
                        {networks.map((n) => (
                          <option key={n.name} value={n.name}>
                            {n.name}
                          </option>
                        ))}
                      </select>
                      <IconChevronDown
                        size={16}
                        className="pointer-events-none absolute right-3 top-1/2 -translate-y-1/2 text-foreground-400"
                      />
                    </div>
                  </div>
                )}
              />
              <Controller
                control={control}
                name="machineName"
                render={({ field }) => (
                  <Input
                    label="Machine name"
                    labelPlacement="outside"
                    placeholder="auto (e.g. db, api)"
                    value={field.value}
                    onValueChange={field.onChange}
                    variant="bordered"
                    isInvalid={!!errors.machineName}
                    errorMessage={errors.machineName?.message}
                    description={
                      watch("network") ? "Its DNS name on the network" : undefined
                    }
                  />
                )}
              />
            </div>

            <button
              type="button"
              onClick={() => setAdvanced((a) => !a)}
              className="flex items-center gap-1 text-xs font-medium text-foreground-500 transition hover:text-foreground-400"
            >
              <IconChevronDown
                size={14}
                className={`transition ${advanced ? "rotate-180" : ""}`}
              />
              Advanced options
            </button>

            {advanced && (
              <div className="flex flex-col gap-4 rounded-xl border border-white/10 bg-content2/40 p-4">
                <Controller
                  control={control}
                  name="noNet"
                  render={({ field }) => (
                    <div className="flex items-center justify-between">
                      <div>
                        <div className="text-sm font-medium">Disable networking</div>
                        <div className="text-xs text-foreground-500">
                          {unikernel
                            ? "Isolated guest (no gvproxy, no NIC)."
                            : "Isolated guest (no gvproxy; exec/shell unavailable)."}
                        </div>
                      </div>
                      <Switch
                        size="sm"
                        isSelected={field.value}
                        onValueChange={field.onChange}
                      />
                    </div>
                  )}
                />

                {kind === "linux" && (
                  <>
                    <Controller
                      control={control}
                      name="initramfs"
                      render={({ field }) => (
                        <div className="flex items-center justify-between">
                          <div>
                            <div className="text-sm font-medium">
                              Boot from initramfs
                            </div>
                            <div className="text-xs text-foreground-500">
                              Load rootfs into RAM instead of virtio-fs.
                            </div>
                          </div>
                          <Switch
                            size="sm"
                            isSelected={field.value}
                            onValueChange={field.onChange}
                          />
                        </div>
                      )}
                    />
                    <Controller
                      control={control}
                      name="entrypoint"
                      render={({ field }) => (
                        <Input
                          label="Entrypoint override"
                          labelPlacement="outside"
                          placeholder="/bin/sh"
                          value={field.value}
                          onValueChange={field.onChange}
                          variant="bordered"
                          classNames={{ input: "font-mono text-xs" }}
                        />
                      )}
                    />
                  </>
                )}

                {/* Both kinds share host directories over virtio-fs. For a
                    unikernel it is the only way to keep anything, so it is
                    labelled as volumes rather than bind mounts. */}
                {(kind === "linux" || unikraft) && (
                  <Controller
                    control={control}
                    name="mounts"
                    render={({ field }) => (
                      <Textarea
                        label={
                          unikraft
                            ? "Volumes (one per line, HOST:GUEST)"
                            : "Bind mounts (one per line)"
                        }
                        labelPlacement="outside"
                        placeholder={
                          unikraft
                            ? "~/data:/data\n~/logs:/var/log"
                            : "~/project:/src\n~/data:/data:ro"
                        }
                        value={field.value}
                        onValueChange={field.onChange}
                        minRows={2}
                        variant="bordered"
                        description={
                          unikraft
                            ? "Shared over virtio-fs and persist across boots. The guest path must be absolute, and the unikernel must be built for it."
                            : undefined
                        }
                        classNames={{ input: "font-mono text-xs" }}
                      />
                    )}
                  />
                )}

                {/* Solo5 block devices: the unikernel declares them in its
                    manifest; the host only supplies the backing files. */}
                {solo5 && (
                  <Controller
                    control={control}
                    name="blocks"
                    render={({ field }) => (
                      <Textarea
                        label="Block devices (one per line, NAME=FILE)"
                        labelPlacement="outside"
                        placeholder={"storage=~/disk.img"}
                        value={field.value}
                        onValueChange={field.onChange}
                        minRows={2}
                        variant="bordered"
                        isInvalid={!!errors.blocks}
                        errorMessage={errors.blocks?.message}
                        description="Backs a block device the unikernel declares. NAME= may be omitted when it declares exactly one."
                        classNames={{ input: "font-mono text-xs" }}
                      />
                    )}
                  />
                )}

                <Controller
                  control={control}
                  name="ports"
                  render={({ field }) => (
                    <Textarea
                      label="Port forwards (one per line, HOST:GUEST)"
                      labelPlacement="outside"
                      placeholder={"8080:80\n2222:22"}
                      value={field.value}
                      onValueChange={field.onChange}
                      minRows={2}
                      variant="bordered"
                      isInvalid={!!errors.ports}
                      errorMessage={errors.ports?.message}
                      classNames={{ input: "font-mono text-xs" }}
                    />
                  )}
                />

                {(bsd || kind === "linux") && (
                  <Controller
                    control={control}
                    name="attachDisks"
                    render={({ field }) => (
                      <Textarea
                        label="Attach disks (one per line, PATH[:ro])"
                        labelPlacement="outside"
                        placeholder={"/path/to/data.raw\n/path/to/ref.img:ro"}
                        value={field.value}
                        onValueChange={field.onChange}
                        minRows={2}
                        variant="bordered"
                        classNames={{ input: "font-mono text-xs" }}
                      />
                    )}
                  />
                )}
              </div>
            )}
          </ModalBody>
          <ModalFooter>
            <Button variant="light" onPress={() => setOpen(false)}>
              Cancel
            </Button>
            <Button
              type="submit"
              color="primary"
              variant="shadow"
              isLoading={isSubmitting}
              startContent={!isSubmitting && <IconRocket size={16} />}
            >
              Run machine
            </Button>
          </ModalFooter>
        </form>
      </ModalContent>
    </Modal>
  );
}
