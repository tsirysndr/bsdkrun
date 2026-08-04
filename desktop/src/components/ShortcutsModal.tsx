import {
  Kbd,
  Modal,
  ModalBody,
  ModalContent,
  ModalHeader,
} from "@heroui/react";
import { useAtom } from "jotai";
import { IconKeyboard } from "@tabler/icons-react";
import { shortcutsOpenAtom } from "../state/atoms";

const GROUPS: { title: string; items: [string[], string][] }[] = [
  {
    title: "General",
    items: [
      [["/"], "Open command palette"],
      [["⌘", "K"], "Open command palette"],
      [["?"], "Show this help"],
      [["R"], "Refresh"],
      [["Esc"], "Close dialog / palette"],
    ],
  },
  {
    title: "Navigate",
    items: [
      [["⌘", "1"], "Machines"],
      [["⌘", "2"], "Images"],
      [["⌘", "3"], "Volumes"],
    ],
  },
  {
    title: "Machines",
    items: [
      [["↑"], "Highlight previous machine"],
      [["↓"], "Highlight next machine"],
      [["↵"], "Open highlighted (Logs)"],
      [["L"], "Open logs of highlighted"],
      [["T"], "Open terminal of highlighted"],
      [["N"], "Run new machine"],
      [["⌘", "⇧", "S"], "Stop all running"],
    ],
  },
  {
    title: "Command palette",
    items: [
      [["↑"], "Previous result"],
      [["↓"], "Next result"],
      [["↵"], "Run highlighted command"],
      [["⌘", "↵"], "Open terminal for highlighted machine"],
    ],
  },
  {
    title: "Terminal & logs",
    items: [
      [["⌃", "`"], "Toggle bottom terminal panel"],
      [["⌘", "C"], "Copy selection (terminal / logs)"],
      [["⌘", "F"], "Search in logs"],
      [["↵"], "Find next in logs (⇧↵ previous)"],
    ],
  },
];

export default function ShortcutsModal() {
  const [open, setOpen] = useAtom(shortcutsOpenAtom);
  return (
    <Modal
      isOpen={open}
      onClose={() => setOpen(false)}
      size="4xl"
      backdrop="opaque"
      shouldBlockScroll={false}
      classNames={{ base: "border border-white/10 bg-content1" }}
    >
      <ModalContent>
        <ModalHeader className="flex items-center gap-2 text-base">
          <IconKeyboard size={18} className="text-primary" />
          Keyboard Shortcuts
        </ModalHeader>
        <ModalBody className="grid grid-cols-1 gap-x-8 gap-y-5 pb-6 sm:grid-cols-2 lg:grid-cols-3">
          {GROUPS.map((g) => (
            <div key={g.title} className="break-inside-avoid">
              <div className="mb-2.5 border-b border-white/10 pb-1.5 text-[11px] font-medium uppercase tracking-wider text-foreground-500">
                {g.title}
              </div>
              <div className="flex flex-col">
                {g.items.map(([keys, label]) => (
                  <div
                    key={label + keys.join()}
                    className="flex items-center justify-between gap-4 rounded-md px-1.5 py-1.5 transition-colors hover:bg-white/5"
                  >
                    <span className="text-[13px] font-light leading-snug text-foreground-400">
                      {label}
                    </span>
                    <span className="flex shrink-0 items-center gap-1">
                      {keys.map((k) => (
                        <Kbd
                          key={k}
                          className="min-w-[24px] justify-center border border-white/10 bg-content2 text-center text-xs font-medium text-foreground"
                        >
                          {k}
                        </Kbd>
                      ))}
                    </span>
                  </div>
                ))}
              </div>
            </div>
          ))}
        </ModalBody>
      </ModalContent>
    </Modal>
  );
}
