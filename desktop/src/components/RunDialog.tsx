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
import { useRunMachine, useVersions } from "../lib/queries";
import { useToast } from "../state/toast";
import type { RunSpec } from "../lib/types";

type Kind = "linux" | "freebsd" | "netbsd";

const KIND_LABEL: Record<Kind, string> = {
  linux: "Linux (OCI)",
  freebsd: "FreeBSD",
  netbsd: "NetBSD",
};

// ---- zod schema ------------------------------------------------------------

const schema = z
  .object({
    kind: z.enum(["linux", "freebsd", "netbsd"]),
    image: z.string(),
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
            .every((l) => /^\d+:\d+$/.test(l)),
        "Each line must be HOST:GUEST (numbers)",
      ),
    attachDisks: z.string(),
    diskSize: z
      .string()
      .regex(/^$|^\d+(\.\d+)?\s*[KMGT]?B?$/i, "e.g. 8G, 4096M"),
  })
  .refine((d) => d.kind !== "linux" || d.image.trim().length > 0, {
    message: "An image reference is required",
    path: ["image"],
  });

type FormValues = z.infer<typeof schema>;

const DEFAULTS: FormValues = {
  kind: "linux",
  image: "alpine",
  version: "",
  cpus: 1,
  mem: 512,
  volume: "",
  command: "",
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
  const runMutation = useRunMachine();
  const toast = useToast();
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
  const bsd = kind !== "linux";
  const version = watch("version");

  const { data: versions = [] } = useVersions(kind, open && bsd);

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

  // Default the BSD version: FreeBSD → 15.1, NetBSD → 11.0 when available,
  // otherwise the release the CLI marks as latest.
  useEffect(() => {
    if (bsd && versions.length && !version) {
      const preferred = kind === "freebsd" ? "15.1" : "11.0";
      const chosen =
        versions.find((v) => v.version === preferred) ||
        versions.find((v) => v.latest) ||
        versions[versions.length - 1];
      if (chosen) setValue("version", chosen.version);
    }
  }, [versions, bsd, version, kind, setValue]);

  const onSubmit = async (data: FormValues) => {
    const spec: RunSpec = {
      kind: data.kind,
      image: data.kind === "linux" ? data.image.trim() : null,
      version: bsd && data.version ? data.version : null,
      cpus: data.cpus,
      mem: data.mem,
      volume: data.volume.trim() || null,
      no_net: data.noNet,
      initramfs: data.kind === "linux" ? data.initramfs : false,
      entrypoint:
        data.kind === "linux" && data.entrypoint.trim()
          ? data.entrypoint.trim()
          : null,
      mounts: data.kind === "linux" ? lines(data.mounts) : [],
      ports: lines(data.ports),
      attach_disks: bsd ? lines(data.attachDisks) : [],
      disk_size: bsd && data.diskSize.trim() ? data.diskSize.trim() : null,
      command: splitArgs(data.command),
    };
    try {
      const id = await runMutation.mutateAsync(spec);
      toast("success", "Machine started", `${KIND_LABEL[data.kind]} · ${id.slice(0, 12)}`);
      setOpen(false);
      reset(DEFAULTS);
      setAdvanced(false);
      setView("machines");
    } catch (e) {
      toast("error", "Failed to start machine", String(e));
    }
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
            ) : (
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
                            {v.version}
                            {v.latest ? "  (latest)" : ""}
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

            <div className="grid grid-cols-3 gap-3">
              <Controller
                control={control}
                name="cpus"
                render={({ field }) => (
                  <Input
                    type="number"
                    label="vCPUs"
                    labelPlacement="outside"
                    value={String(field.value)}
                    onValueChange={field.onChange}
                    min={1}
                    variant="bordered"
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
            </div>

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

            <button
              type="button"
              onClick={() => setAdvanced((a) => !a)}
              className="flex items-center gap-1 text-xs font-medium text-foreground-500 transition hover:text-foreground-300"
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
                          Isolated guest (no gvproxy; exec/shell unavailable).
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
                    <Controller
                      control={control}
                      name="mounts"
                      render={({ field }) => (
                        <Textarea
                          label="Bind mounts (one per line)"
                          labelPlacement="outside"
                          placeholder={"~/project:/src\n~/data:/data:ro"}
                          value={field.value}
                          onValueChange={field.onChange}
                          minRows={2}
                          variant="bordered"
                          classNames={{ input: "font-mono text-xs" }}
                        />
                      )}
                    />
                  </>
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

                {bsd && (
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
