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
});
type FormValues = z.infer<typeof schema>;

export default function SettingsModal() {
  const [open, setOpen] = useAtom(settingsOpenAtom);
  const { data: settings } = useSettings();
  const { data: defaultCachePath } = useDefaultCache();
  const { data: probe, refetch: refetchProbe, isFetching: checking } = useProbe();
  const saveMutation = useSaveSettings();
  const toast = useToast();

  const { control, handleSubmit, reset, setValue } = useForm<FormValues>({
    resolver: zodResolver(schema),
    defaultValues: { binaryPath: "", cachePath: "" },
  });

  useEffect(() => {
    if (open)
      reset({
        binaryPath: settings?.binary_path || "",
        cachePath: settings?.cache_path || "",
      });
  }, [open, settings?.binary_path, settings?.cache_path, reset]);

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
                bsdkrun binary path
              </label>
              <div className="flex gap-2">
                <Controller
                  control={control}
                  name="binaryPath"
                  render={({ field }) => (
                    <Input
                      value={field.value}
                      onValueChange={field.onChange}
                      placeholder="Auto-detected from PATH / Homebrew"
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
                Leave empty to auto-resolve. bsdkrun runs the microVMs; this GUI
                just drives it.
              </p>
            </div>

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
