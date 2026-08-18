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
import {
  IconSettings,
  IconCircleCheck,
  IconAlertTriangle,
  IconPlugConnected,
} from "@tabler/icons-react";
import { settingsOpenAtom,
  themeAtom,
} from "../state/atoms";
import { useProbe, useSaveSettings, useSettings } from "../lib/queries";
import { useToast } from "../state/toast";
import { clearConnection, DEFAULT_URL } from "../lib/connection";

// Where the desktop app configured a local binary and cache directory, the web
// app configures which daemon to talk to — it is served from anywhere and
// cannot discover its own backend.
const schema = z.object({
  url: z.string().min(1, "The API URL is required"),
  token: z.string().min(1, "The access token is required"),
});
type FormValues = z.infer<typeof schema>;

export default function SettingsModal() {
  const [theme, setTheme] = useAtom(themeAtom);
  const [open, setOpen] = useAtom(settingsOpenAtom);
  const { data: settings } = useSettings();
  const { data: probe, refetch: refetchProbe, isFetching: checking } = useProbe();
  const saveMutation = useSaveSettings();
  const toast = useToast();

  const {
    control,
    handleSubmit,
    reset,
    formState: { errors },
  } = useForm<FormValues>({
    resolver: zodResolver(schema),
    defaultValues: { url: "", token: "" },
  });

  useEffect(() => {
    if (open) reset({ url: settings?.url || "", token: settings?.token || "" });
  }, [open, settings?.url, settings?.token, reset]);

  const onSubmit = async (data: FormValues) => {
    try {
      await saveMutation.mutateAsync({ url: data.url, token: data.token });
      toast("success", "Connection saved");
      await refetchProbe();
    } catch (e) {
      toast("error", "Failed to save", String(e));
    }
  };

  const disconnect = () => {
    clearConnection();
    setOpen(false);
    // A full reload is the honest way to drop every cached query, open
    // subscription and terminal belonging to the daemon we just left.
    window.location.reload();
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
            {/* Appearance first: it is the setting people come looking for. */}
            <div>
              <label className="mb-1.5 block text-sm font-medium">Appearance</label>
              <div className="flex gap-2">
                {(
                  [
                    ["night-rider", "Night Rider"],
                    ["dark", "Classic Dark"],
                  ] as const
                ).map(([key, label]) => (
                  <button
                    key={key}
                    onClick={() => setTheme(key)}
                    className={`rounded-lg border px-3 py-1.5 text-sm transition ${
                      theme === key
                        ? "border-primary/60 bg-primary/15 text-primary"
                        : "border-default-200 text-foreground-500 hover:bg-default-100/70"
                    }`}
                  >
                    {label}
                  </button>
                ))}
              </div>
              <p className="mt-1 text-[11px] text-foreground-500">
                Also: the Appearance button in the sidebar, the command palette, or the{" "}
                <kbd className="rounded bg-default-100 px-1">t</kbd> key.
              </p>
            </div>
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
                  {probe?.ok ? "Daemon reachable" : "Daemon not reachable"}
                </div>
                <div className="mt-0.5 break-words text-xs text-foreground-500">
                  {probe?.message || "Save a connection to check it."}
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
              <label className="mb-1.5 block text-sm font-medium">GraphQL API URL</label>
              <Controller
                control={control}
                name="url"
                render={({ field }) => (
                  <Input
                    value={field.value}
                    onValueChange={field.onChange}
                    placeholder={DEFAULT_URL}
                    variant="bordered"
                    isInvalid={!!errors.url}
                    errorMessage={errors.url?.message}
                    classNames={{ input: "font-mono text-xs" }}
                  />
                )}
              />
              <p className="mt-1.5 text-xs text-foreground-500">
                Where <code className="font-mono">bsdkrund</code> serves GraphQL. A bare
                host like <span className="font-mono">localhost:50052</span> is fine —{" "}
                <span className="font-mono">/graphql</span> is added for you.
              </p>
            </div>

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
                    isInvalid={!!errors.token}
                    errorMessage={errors.token?.message}
                    classNames={{ input: "font-mono text-xs" }}
                  />
                )}
              />
              <p className="mt-1.5 text-xs text-foreground-500">
                Printed by the daemon on startup, or whatever you set as{" "}
                <code className="font-mono">BSDKRUN_TOKEN</code>. Stored in this
                browser's local storage.
              </p>
            </div>
          </ModalBody>
          <ModalFooter className="justify-between">
            <Button
              variant="light"
              color="danger"
              startContent={<IconPlugConnected size={16} />}
              onPress={disconnect}
            >
              Disconnect
            </Button>
            <div className="flex gap-2">
              <Button variant="light" onPress={() => setOpen(false)}>
                Close
              </Button>
              <Button type="submit" color="primary" isLoading={saveMutation.isPending}>
                Save
              </Button>
            </div>
          </ModalFooter>
        </form>
      </ModalContent>
    </Modal>
  );
}
