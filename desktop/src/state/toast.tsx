import { atom, useAtom, useSetAtom } from "jotai";
import { useCallback } from "react";
import {
  IconCircleCheck,
  IconExclamationCircle,
  IconInfoCircle,
  IconX,
} from "@tabler/icons-react";

export type ToastKind = "success" | "error" | "info";
export interface Toast {
  id: number;
  kind: ToastKind;
  title: string;
  detail?: string;
}

const toastsAtom = atom<Toast[]>([]);
let seq = 1;

/** Imperative-ish toast API returned as a hook. */
export function useToast() {
  const setToasts = useSetAtom(toastsAtom);
  return useCallback(
    (kind: ToastKind, title: string, detail?: string) => {
      const id = seq++;
      setToasts((t) => [...t, { id, kind, title, detail }]);
      setTimeout(() => {
        setToasts((t) => t.filter((x) => x.id !== id));
      }, kind === "error" ? 6000 : 3200);
    },
    [setToasts],
  );
}

const ICON = {
  success: IconCircleCheck,
  error: IconExclamationCircle,
  info: IconInfoCircle,
};
const ACCENT = {
  success: "border-l-emerald-400 text-emerald-300",
  error: "border-l-red-400 text-red-300",
  info: "border-l-sky-400 text-sky-300",
};

export function Toaster() {
  const [toasts, setToasts] = useAtom(toastsAtom);
  return (
    <div className="pointer-events-none fixed bottom-5 right-5 z-[200] flex flex-col gap-2">
      {toasts.map((t) => {
        const Icon = ICON[t.kind];
        return (
          <div
            key={t.id}
            className={`pointer-events-auto flex w-80 items-start gap-3 rounded-xl border border-white/10 border-l-4 bg-content1/95 px-4 py-3 shadow-2xl ${ACCENT[t.kind]}`}
          >
            <Icon size={20} className="mt-0.5 shrink-0" />
            <div className="min-w-0 flex-1">
              <div className="text-sm font-medium text-foreground">
                {t.title}
              </div>
              {t.detail && (
                <div className="mt-0.5 break-words text-xs text-foreground-500">
                  {t.detail}
                </div>
              )}
            </div>
            <button
              className="text-foreground-400 transition hover:text-foreground"
              onClick={() =>
                setToasts((cur) => cur.filter((x) => x.id !== t.id))
              }
            >
              <IconX size={16} />
            </button>
          </div>
        );
      })}
    </div>
  );
}
