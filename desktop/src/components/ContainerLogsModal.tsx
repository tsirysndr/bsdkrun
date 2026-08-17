import { useEffect, useRef, useState } from "react";
import {
  Modal,
  ModalBody,
  ModalContent,
  ModalHeader,
} from "@heroui/react";
import { IconFileText, IconRefresh } from "@tabler/icons-react";
import { api } from "../lib/api";
import type { DockerContainer } from "../lib/types";

/**
 * One container's logs.
 *
 * Polled rather than streamed: the Docker API's log stream is a hijacked
 * connection with its own framing, and re-reading the tail every couple of
 * seconds is indistinguishable at this size — and cannot leave a socket open
 * behind a closed modal.
 */
export default function ContainerLogsModal({
  container,
  onClose,
}: {
  container: DockerContainer | null;
  onClose: () => void;
}) {
  const [text, setText] = useState("");
  const [error, setError] = useState<string | null>(null);
  const [loading, setLoading] = useState(false);
  const bodyRef = useRef<HTMLPreElement>(null);
  const stick = useRef(true);

  useEffect(() => {
    if (!container) {
      setText("");
      setError(null);
      return;
    }
    let alive = true;
    const load = async () => {
      setLoading(true);
      try {
        const logs = await api.dockerLogs(container.id, 500);
        if (!alive) return;
        setText(logs);
        setError(null);
      } catch (e) {
        if (alive) setError(String(e));
      } finally {
        if (alive) setLoading(false);
      }
    };
    load();
    const t = setInterval(load, 2500);
    return () => {
      alive = false;
      clearInterval(t);
    };
  }, [container]);

  // Follow the tail, unless the reader has scrolled up to look at something.
  useEffect(() => {
    const el = bodyRef.current;
    if (el && stick.current) el.scrollTop = el.scrollHeight;
  }, [text]);

  return (
    <Modal
      isOpen={container !== null}
      onClose={onClose}
      size="4xl"
      backdrop="opaque"
      shouldBlockScroll={false}
      classNames={{ base: "border border-white/10 bg-content1" }}
    >
      <ModalContent>
        <ModalHeader className="flex items-center gap-2 text-base">
          <IconFileText size={18} className="text-foreground-400" />
          <span className="truncate">{container?.name || container?.id}</span>
          <span className="font-mono text-xs font-normal text-foreground-400">
            {container?.image}
          </span>
          <div className="flex-1" />
          {loading && (
            <IconRefresh size={14} className="animate-spin text-foreground-500" />
          )}
        </ModalHeader>
        <ModalBody className="pb-5">
          {error ? (
            <div className="rounded-lg border border-danger/20 bg-danger/5 px-3 py-2 text-xs text-danger">
              {error}
            </div>
          ) : (
            <pre
              ref={bodyRef}
              onScroll={(e) => {
                const el = e.currentTarget;
                stick.current =
                  el.scrollHeight - el.scrollTop - el.clientHeight < 40;
              }}
              // 13px/1.6 on a near-black panel: the xterm log view already
              // runs at 14px, and 11px grey was unreadable next to it.
              className="h-[60vh] select-text overflow-auto whitespace-pre-wrap break-all rounded-lg bg-[#0a0d13] p-4 font-mono text-[13px] leading-[1.6] text-foreground-100"
            >
              {text || (loading ? "Loading…" : "(no output yet)")}
            </pre>
          )}
        </ModalBody>
      </ModalContent>
    </Modal>
  );
}
