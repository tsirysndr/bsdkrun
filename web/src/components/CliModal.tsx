import {
  Modal,
  ModalBody,
  ModalContent,
  ModalHeader,
  Snippet,
} from "@heroui/react";
import { useAtom } from "jotai";
import { IconTerminal2, IconExternalLink } from "@tabler/icons-react";
import { cliModalOpenAtom } from "../state/atoms";

/** Open a link in a new tab. `noopener` so the page cannot reach back via window.opener. */
const openExternal = (url: string) => {
  window.open(url, "_blank", "noopener,noreferrer");
  return Promise.resolve();
};

const SECTIONS: { title: string; note?: string; commands: string[] }[] = [
  {
    title: "macOS (Apple Silicon) — Homebrew",
    note: "Ships codesigned with the hypervisor entitlement; nothing else to set up.",
    commands: ["brew install tsirysndr/tap/bsdkrun"],
  },
  {
    title: "npm — prebuilt binary",
    note: "macOS arm64 · Linux x64 / arm64. Bundles libkrun on Linux.",
    commands: [
      "npm install -g @bsdkrun/cli",
      "npx @bsdkrun/cli linux alpine -- echo hi",
    ],
  },
  {
    title: "Nix flake",
    note: "Linux needs /dev/kvm access; macOS links Homebrew's libkrun (--impure).",
    commands: [
      "nix run github:tsirysndr/bsdkrun -- linux alpine",
      "nix profile install github:tsirysndr/bsdkrun",
    ],
  },
];

export default function CliModal() {
  const [open, setOpen] = useAtom(cliModalOpenAtom);
  return (
    <Modal
      isOpen={open}
      onClose={() => setOpen(false)}
      size="xl"
      backdrop="opaque"
      scrollBehavior="inside"
      shouldBlockScroll={false}
      classNames={{ base: "border border-white/10 bg-content1" }}
    >
      <ModalContent>
        <ModalHeader className="flex flex-col gap-1">
          <div className="flex items-center gap-2 text-base">
            <IconTerminal2 size={18} className="text-primary" />
            Install the bsdkrun CLI
          </div>
          <p className="text-xs font-normal text-foreground-500">
            This app drives the <span className="font-mono">bsdkrun</span> CLI —
            install it once, then point Settings at it if it isn't auto-detected.
          </p>
        </ModalHeader>
        <ModalBody className="gap-5 pb-6">
          {SECTIONS.map((s) => (
            <div key={s.title}>
              <div className="mb-1.5 text-sm font-medium">{s.title}</div>
              {s.note && (
                <p className="mb-2 text-xs text-foreground-500">{s.note}</p>
              )}
              <div className="flex flex-col gap-2">
                {s.commands.map((c) => (
                  <Snippet
                    key={c}
                    size="sm"
                    variant="bordered"
                    symbol="$"
                    classNames={{
                      base: "border-white/10 bg-content2/50 w-full",
                      pre: "font-mono text-[13px] leading-[1.6] whitespace-pre-wrap select-text",
                    }}
                  >
                    {c}
                  </Snippet>
                ))}
              </div>
            </div>
          ))}

          <button
            onClick={() =>
              openExternal("https://github.com/tsirysndr/bsdkrun#install").catch(
                () => {},
              )
            }
            className="flex items-center gap-1.5 text-xs font-medium text-primary-400 transition hover:text-primary-300"
          >
            <IconExternalLink size={14} />
            Full install guide &amp; prerequisites on GitHub
          </button>
        </ModalBody>
      </ModalContent>
    </Modal>
  );
}
