import {
  Button,
  Modal,
  ModalBody,
  ModalContent,
  ModalFooter,
  ModalHeader,
} from "@heroui/react";
import { useEffect, useState, type ReactNode } from "react";

export function ConfirmDialog({
  open,
  title,
  body,
  confirmLabel = "Confirm",
  danger,
  onConfirm,
  onClose,
}: {
  open: boolean;
  title: string;
  body: ReactNode;
  confirmLabel?: string;
  danger?: boolean;
  onConfirm: () => void | Promise<void>;
  onClose: () => void;
}) {
  // Show a spinner the instant Confirm is clicked, until the action settles
  // (or the dialog closes). onConfirm may be sync or async.
  const [busy, setBusy] = useState(false);
  useEffect(() => {
    if (!open) setBusy(false);
  }, [open]);

  const confirm = () => {
    setBusy(true);
    Promise.resolve(onConfirm()).finally(() => setBusy(false));
  };

  return (
    <Modal
      isOpen={open}
      onClose={() => {
        if (!busy) onClose();
      }}
      size="sm"
      backdrop="opaque"
      shouldBlockScroll={false}
      classNames={{ base: "border border-white/10 bg-content1" }}
    >
      <ModalContent>
        <ModalHeader className="text-base">{title}</ModalHeader>
        <ModalBody className="text-sm text-foreground-400">{body}</ModalBody>
        <ModalFooter>
          <Button variant="light" size="sm" isDisabled={busy} onPress={onClose}>
            Cancel
          </Button>
          <Button
            size="sm"
            color={danger ? "danger" : "primary"}
            isLoading={busy}
            onPress={confirm}
          >
            {confirmLabel}
          </Button>
        </ModalFooter>
      </ModalContent>
    </Modal>
  );
}
