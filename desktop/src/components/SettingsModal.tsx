import { useEffect } from "react";
import {
  Button,
  Input,
  Modal,
  ModalBody,
  ModalContent,
  ModalFooter,
  ModalHeader,
} from "@heroui/react";
import { useAtom } from "jotai";
import { Controller, useForm } from "react-hook-form";
import { zodResolver } from "@hookform/resolvers/zod";
import { z } from "zod";
import { open as openDialog } from "@tauri-apps/plugin-dialog";
import {
  IconSettings,
  IconFolderOpen,
  IconCircleCheck,
  IconAlertTriangle,
} from "@tabler/icons-react";
import { settingsOpenAtom } from "../state/atoms";
import {
  useDefaultCache,
  useProbe,
  useSaveSettings,
  useSettings,
} from "../lib/queries";
import { useToast } from "../state/toast";

const schema = z.object({
  binaryPath: z.string(),
  cachePath: z.string(),
  token: z.string(),
});

/**
 * Does this target look like a daemon URL rather than a local binary path?
 *
 * Mirrors the Rust side (src-tauri/src/target.rs) exactly: only an explicit
 * scheme counts, because a bare `host:50051` is ambiguous with a relative path.
 */
const looksLikeUrl = (s: string) =>
  /^(grpc|grpcs|http|https):\/\/.+/i.test(s.trim());
type FormValues = z.infer<typeof schema>;

export default function SettingsModal() {
  const [open, setOpen] = useAtom(settingsOpenAtom);
  const { data: settings } = useSettings();
  const { data: defaultCachePath } = useDefaultCache();
  const { data: probe, refetch: refetchProbe, isFetching: checking } = useProbe();
  const saveMutation = useSaveSettings();
  const toast = useToast();

  const { control, handleSubmit, reset, setValue, watch } = useForm<FormValues>({
    resolver: zodResolver(schema),
    defaultValues: { binaryPath: "", cachePath: "", token: "" },
  });

  useEffect(() => {
    if (open)
      reset({
        binaryPath: settings?.binary_path || "",
        cachePath: settings?.cache_path || "",
        token: settings?.token || "",
      });
  }, [open, settings?.binary_path, settings?.cache_path, settings?.token, reset]);

  // The token field only makes sense for a daemon, so it appears with one.
  const isRemote = looksLikeUrl(watch("binaryPath") || "");

  const browse = async () => {
    const picked = await openDialog({ multiple: false, directory: false });
    if (typeof picked === "string") setValue("binaryPath", picked);
  };

  const browseCache = async () => {
    const picked = await openDialog({ multiple: false, directory: true });
    if (typeof picked === "string") setValue("cachePath", picked);
  };

  const onSubmit = async (data: FormValues) => {
    try {
      await saveMutation.mutateAsync({
        binaryPath: data.binaryPath.trim(),
        cachePath: data.cachePath.trim(),
        token: data.token.trim(),
      });
      toast("success", "Settings saved");
      await refetchProbe();
    } catch (e) {
      toast("error", "Failed to save", String(e));
    }
  };

  return (
    <Modal
      isOpen={open}
      onClose={() => setOpen(false)}
      size="xl"
      backdrop="opaque"
      shouldBlockScroll={false}
      classNames={{ base: "border border-white/10 bg-content1" }}
    >
      <ModalContent>
        <form onSubmit={handleSubmit(onSubmit)}>
          <ModalHeader className="flex items-center gap-2 text-base">
            <IconSettings size={18} className="text-primary" />
            Settings
          </ModalHeader>
          <ModalBody className="gap-5">
            <div
              className={`flex items-start gap-3 rounded-xl border p-4 ${
                probe?.ok
                  ? "border-emerald-500/20 bg-emerald-500/5"
                  : "border-amber-500/20 bg-amber-500/5"
              }`}
            >
              {probe?.ok ? (
                <IconCircleCheck size={20} className="mt-0.5 text-emerald-400" />
              ) : (
                <IconAlertTriangle size={20} className="mt-0.5 text-amber-400" />
              )}
              <div className="min-w-0 flex-1">
                <div className="text-sm font-medium">
                  {probe?.ok ? "Engine reachable" : "Engine not ready"}
                </div>
                <div className="mt-0.5 break-words text-xs text-foreground-500">
                  {probe?.message || "Run a probe to check the toolchain."}
                </div>
                {probe?.binary && (
                  <div className="mt-1 break-all font-mono text-[11px] text-foreground-600">
                    {probe.binary}
                  </div>
                )}
              </div>
              <Button
                size="sm"
                variant="flat"
                isLoading={checking}
                onPress={() => refetchProbe()}
              >
                Re-check
              </Button>
            </div>

            <div>
              <label className="mb-1.5 block text-sm font-medium">
                bsdkrun binary path or daemon URL
              </label>
              <div className="flex gap-2">
                <Controller
                  control={control}
                  name="binaryPath"
                  render={({ field }) => (
                    <Input
                      value={field.value}
                      onValueChange={field.onChange}
                      placeholder="Auto-detected from PATH, or grpc://host:50051"
                      variant="bordered"
                      classNames={{ input: "font-mono text-xs" }}
                    />
                  )}
                />
                <Button
                  type="button"
                  variant="flat"
                  isIconOnly
                  onPress={browse}
                  aria-label="Browse"
                >
                  <IconFolderOpen size={18} />
                </Button>
              </div>
              <p className="mt-1.5 text-xs text-foreground-500">
                {isRemote ? (
                  <>
                    Driving a remote <code className="font-mono">bsdkrund</code>. Every
                    action — including terminals and logs — runs on that host, so the
                    VMs live there, not here.
                  </>
                ) : (
                  <>
                    Leave empty to auto-resolve. bsdkrun runs the microVMs; this GUI
                    just drives it. Enter a{" "}
                    <code className="font-mono">grpc://host:50051</code> URL instead to
                    drive a remote daemon.
                  </>
                )}
              </p>
            </div>

            {isRemote && (
              <div>
                <label className="mb-1.5 block text-sm font-medium">Access token</label>
                <Controller
                  control={control}
                  name="token"
                  render={({ field }) => (
                    <Input
                      value={field.value}
                      onValueChange={field.onChange}
                      placeholder="the token bsdkrund printed on startup"
                      variant="bordered"
                      type="password"
                      classNames={{ input: "font-mono text-xs" }}
                    />
                  )}
                />
                <p className="mt-1.5 text-xs text-foreground-500">
                  Falls back to <code className="font-mono">$BSDKRUN_TOKEN</code> when
                  left empty.
                </p>
              </div>
            )}

            <div>
              <label className="mb-1.5 block text-sm font-medium">
                Cache directory
              </label>
              <div className="flex gap-2">
                <Controller
                  control={control}
                  name="cachePath"
                  render={({ field }) => (
                    <Input
                      value={field.value}
                      onValueChange={field.onChange}
                      placeholder={defaultCachePath || "~/.cache/bsdkrun"}
                      variant="bordered"
                      classNames={{ input: "font-mono text-xs" }}
                    />
                  )}
                />
                <Button
                  type="button"
                  variant="flat"
                  isIconOnly
                  onPress={browseCache}
                  aria-label="Browse for a cache folder"
                >
                  <IconFolderOpen size={18} />
                </Button>
              </div>
              <p className="mt-1.5 text-xs text-foreground-500">
                {isRemote
                  ? "Ignored for a remote daemon — the cache lives on that host, configured there. "
                  : ""}
                Where bsdkrun stores pulled images, kernels, the agent and flavor
                builds (<code className="font-mono">$BSDKRUN_CACHE</code>). Leave
                empty for the default
                {defaultCachePath ? (
                  <>
                    {" "}
                    <span className="font-mono text-foreground-400">
                      {defaultCachePath}
                    </span>
                  </>
                ) : null}
                .
              </p>
            </div>
          </ModalBody>
          <ModalFooter>
            <Button variant="light" onPress={() => setOpen(false)}>
              Close
            </Button>
            <Button type="submit" color="primary" isLoading={saveMutation.isPending}>
              Save
            </Button>
          </ModalFooter>
        </form>
      </ModalContent>
    </Modal>
  );
}
