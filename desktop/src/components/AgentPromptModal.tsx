import { useEffect, useState } from "react";
import { Modal, ModalContent, Kbd } from "@heroui/react";
import type { Icon } from "@tabler/icons-react";

/**
 * A one-line prompt in the same chrome as the command palette: an icon, a
 * borderless input, ↵ to accept and esc to cancel.
 *
 * Exists because `window.prompt` blocks the whole webview, cannot be styled,
 * and looks like a different application — three reasons that all matter when
 * the thing being asked for is part of a flow (clone this repo, name this
 * session) rather than an interruption.
 */
export default function AgentPromptModal({
  open,
  title,
  placeholder,
  icon: IconComponent,
  hint,
  initialValue = "",
  submitLabel = "↵ confirm",
  allowEmpty = false,
  onSubmit,
  onClose,
}: {
  open: boolean;
  title: string;
  placeholder: string;
  icon: Icon;
  /** A line under the input — what the value will do. */
  hint?: string;
  initialValue?: string;
  submitLabel?: string;
  /** Accept an empty value (naming is optional; a repo URL is not). */
  allowEmpty?: boolean;
  onSubmit: (value: string) => void;
  onClose: () => void;
}) {
  const [value, setValue] = useState(initialValue);

  useEffect(() => {
    if (open) setValue(initialValue);
  }, [open, initialValue]);

  const submit = () => {
    const trimmed = value.trim();
    if (!trimmed && !allowEmpty) return;
    onSubmit(trimmed);
    onClose();
  };

  return (
    <Modal
      isOpen={open}
      onClose={onClose}
      hideCloseButton
      backdrop="opaque"
      size="xl"
      placement="top"
      shouldBlockScroll={false}
      classNames={{
        base: "border border-white/10 bg-content1/95 mt-[12vh]",
        body: "p-0",
      }}
    >
      <ModalContent>
        <div
          onKeyDown={(e) => {
            if (e.key === "Enter") {
              e.preventDefault();
              submit();
            }
          }}
        >
          <div className="flex items-center gap-3 border-b border-white/10 px-4 py-3">
            <IconComponent size={18} className="text-foreground-400" />
            <input
              autoFocus
              value={value}
              onChange={(e) => setValue(e.target.value)}
              placeholder={placeholder}
              spellCheck={false}
              autoCapitalize="off"
              autoCorrect="off"
              className="flex-1 bg-transparent text-sm text-foreground outline-none placeholder:text-foreground-500"
            />
            <Kbd className="bg-content2/60 text-foreground-400">esc</Kbd>
          </div>
          <div className="flex items-center gap-3 px-4 py-2.5 text-[11px] text-foreground-500">
            <span className="flex-1">{hint ?? title}</span>
            <span className="text-foreground-600">{submitLabel}</span>
          </div>
        </div>
      </ModalContent>
    </Modal>
  );
}
