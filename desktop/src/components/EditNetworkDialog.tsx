import {
  Button,
  Modal,
  ModalBody,
  ModalContent,
  ModalFooter,
  ModalHeader,
} from "@heroui/react";
import { useAtom } from "jotai";
import { useEffect, useState } from "react";
import {
  IconChevronDown,
  IconInfoCircle,
  IconNetwork,
} from "@tabler/icons-react";
import { editNetworkAtom } from "../state/atoms";
import { useNetworks, useUpdateMachineNetwork } from "../lib/queries";
import { useToast } from "../state/toast";

/**
 * Edit a machine's global-network membership (join / switch / leave). A VM's
 * NIC is fixed at boot, so the change applies on the next start — surfaced
 * clearly for a running machine.
 */
export default function EditNetworkDialog() {
  const [target, setTarget] = useAtom(editNetworkAtom);
  const { data: networks = [] } = useNetworks();
  const update = useUpdateMachineNetwork();
  const toast = useToast();
  const [value, setValue] = useState("");

  useEffect(() => {
    if (target) setValue(target.network ?? "");
  }, [target]);

  const close = () => setTarget(null);
  const changed = (target?.network ?? "") !== value;

  const onSubmit = async () => {
    if (!target) return;
    const next = value === "" ? null : value;
    try {
      await update.mutateAsync({ id: target.id, network: next });
      const dest = next ? `network “${next}”` : "isolated (no network)";
      toast(
        "success",
        "Network updated",
        target.running
          ? `${target.label} → ${dest} — restart to apply`
          : `${target.label} → ${dest}`,
      );
      setTarget(null);
    } catch (e) {
      toast("error", "Couldn't update network", String(e));
    }
  };

  return (
    <Modal
      isOpen={target !== null}
      onClose={() => {
        if (!update.isPending) close();
      }}
      size="md"
      backdrop="opaque"
      shouldBlockScroll={false}
      classNames={{ base: "border border-white/10 bg-content1" }}
    >
      <ModalContent>
        <ModalHeader className="flex items-center gap-2 text-base">
          <IconNetwork size={18} className="text-primary" />
          Edit network
        </ModalHeader>
        <ModalBody className="gap-4">
          <div>
            <label className="mb-1.5 block text-sm">Network</label>
            <div className="relative">
              {/* Native select — HeroUI's Select popover freezes WKWebView. */}
              <select
                value={value}
                onChange={(e) => setValue(e.target.value)}
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
            <p className="mt-1.5 text-xs text-foreground-500">
              Join a shared network to reach peers by name, or detach back to an
              isolated stack.
            </p>
          </div>
          {target?.running && changed && (
            <div className="flex items-start gap-2 rounded-lg border border-amber-500/20 bg-amber-500/5 px-3 py-2 text-xs text-amber-300">
              <IconInfoCircle size={15} className="mt-0.5 shrink-0" />
              <span>
                {target.label} is running. The network change takes effect the
                next time it's started (stop &amp; start to apply).
              </span>
            </div>
          )}
        </ModalBody>
        <ModalFooter>
          <Button
            variant="light"
            size="sm"
            isDisabled={update.isPending}
            onPress={close}
          >
            Cancel
          </Button>
          <Button
            size="sm"
            color="primary"
            isLoading={update.isPending}
            isDisabled={!changed}
            onPress={onSubmit}
          >
            Save
          </Button>
        </ModalFooter>
      </ModalContent>
    </Modal>
  );
}
