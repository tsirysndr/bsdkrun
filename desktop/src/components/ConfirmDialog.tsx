import {
  Button,
  Modal,
  ModalBody,
  ModalContent,
  ModalFooter,
  ModalHeader,
} from "@heroui/react";
import type { ReactNode } from "react";

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
  onConfirm: () => void;
  onClose: () => void;
}) {
  return (
    <Modal
      isOpen={open}
      onClose={onClose}
      size="sm"
      backdrop="opaque"
      shouldBlockScroll={false}
      classNames={{ base: "border border-white/10 bg-content1" }}
    >
      <ModalContent>
        <ModalHeader className="text-base">{title}</ModalHeader>
        <ModalBody className="text-sm text-foreground-400">{body}</ModalBody>
        <ModalFooter>
          <Button variant="light" size="sm" onPress={onClose}>
            Cancel
          </Button>
          <Button
            size="sm"
            color={danger ? "danger" : "primary"}
            onPress={() => {
              onConfirm();
            }}
          >
            {confirmLabel}
          </Button>
        </ModalFooter>
      </ModalContent>
    </Modal>
  );
}
