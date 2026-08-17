import { useEffect, useRef } from "react";
import {
  Button,
  Modal,
  ModalBody,
  ModalContent,
  ModalFooter,
  ModalHeader,
  Spinner,
} from "@heroui/react";
import { useAtom, useSetAtom } from "jotai";
import {
  IconCircleCheckFilled,
  IconExclamationCircle,
  IconRocket,
  IconTerminal2,
} from "@tabler/icons-react";
import {
  launchStateAtom,
  selectedMachineAtom,
  viewAtom,
} from "../state/atoms";
import { onFlavorDone, onFlavorLog } from "../lib/api";
import { useRefreshAll } from "../lib/queries";
import { shortId } from "../lib/format";

/**
 * Live progress for a streaming flavor launch. Opens instantly on Launch and
 * shows the CLI's pull / provisioning-build / boot output line-by-line, then a
 * success (machine id) or error result. Subscribes to `flavor://*` events and
 * matches them to the active launch by id.
 */
export default function LaunchProgressModal() {
  const [launch, setLaunch] = useAtom(launchStateAtom);
  const setView = useSetAtom(viewAtom);
  const setSelected = useSetAtom(selectedMachineAtom);
  const refreshAll = useRefreshAll();
  const logRef = useRef<HTMLDivElement>(null);

  // Subscribe once; updates the active launch by matching launch_id.
  useEffect(() => {
    const unlisteners: Array<() => void> = [];
    onFlavorLog((p) => {
      setLaunch((s) =>
        s && s.launchId === p.launch_id
          ? { ...s, lines: [...s.lines, p.line] }
          : s,
      );
    }).then((un) => unlisteners.push(un));
    onFlavorDone((p) => {
      setLaunch((s) => {
        if (!s || s.launchId !== p.launch_id) return s;
        return {
          ...s,
          status: p.error ? "error" : "done",
          machineId: p.id,
          error: p.error,
        };
      });
      refreshAll();
    }).then((un) => unlisteners.push(un));
    return () => unlisteners.forEach((un) => un());
  }, [setLaunch, refreshAll]);

  // Auto-scroll to the newest line.
  useEffect(() => {
    const el = logRef.current;
    if (el) el.scrollTop = el.scrollHeight;
  }, [launch?.lines.length, launch?.status]);

  if (!launch) return null;
  const running = launch.status === "running";
  const build = launch.mode === "build";

  const close = () => setLaunch(null);
  const goToMachine = () => {
    if (launch.machineId) setSelected(launch.machineId);
    setView("machines");
    setLaunch(null);
  };

  return (
    <Modal
      isOpen
      onClose={() => {
        // While running, closing just backgrounds it (the launch keeps going).
        close();
      }}
      size="2xl"
      backdrop="opaque"
      shouldBlockScroll={false}
      hideCloseButton={running}
      classNames={{ base: "border border-white/10 bg-content1" }}
    >
      <ModalContent>
        <ModalHeader className="flex items-center gap-2.5 text-base">
          {running ? (
            <Spinner size="sm" color="primary" />
          ) : launch.status === "done" ? (
            <IconCircleCheckFilled size={20} className="text-emerald-400" />
          ) : (
            <IconExclamationCircle size={20} className="text-danger" />
          )}
          <span className="flex items-center gap-2">
            <IconRocket size={16} className="text-foreground-400" />
            {running
              ? build
                ? `Building ${launch.name}…`
                : `Launching ${launch.name}…`
              : launch.status === "done"
                ? build
                  ? `${launch.name} built`
                  : `${launch.name} started`
                : build
                  ? `Build of ${launch.name} failed`
                  : `Launch of ${launch.name} failed`}
          </span>
          {launch.status === "done" && launch.machineId && (
            <span className="ml-1 font-mono text-xs text-foreground-500">
              {shortId(launch.machineId)}
            </span>
          )}
        </ModalHeader>
        <ModalBody className="pb-2">
          <div
            ref={logRef}
            className="h-72 select-text overflow-auto rounded-lg border border-white/10 bg-black/40 p-4 font-mono text-[13px] leading-[1.6] text-foreground-100"
          >
            {launch.lines.length === 0 ? (
              <div className="flex items-center gap-2 text-foreground-500">
                <IconTerminal2 size={14} />
                {build
                  ? "Building… pulling the base image and running the provisioning steps (cached for launches)."
                  : "Starting… first run pulls the base image and builds the environment (this is cached for next time)."}
              </div>
            ) : (
              launch.lines.map((l, i) => (
                <div key={i} className="whitespace-pre-wrap break-words">
                  {l}
                </div>
              ))
            )}
            {launch.error && (
              <div className="mt-2 whitespace-pre-wrap break-words text-danger">
                {launch.error}
              </div>
            )}
          </div>
        </ModalBody>
        <ModalFooter>
          {running ? (
            <Button variant="light" size="sm" onPress={close}>
              {build ? "Build in background" : "Run in background"}
            </Button>
          ) : launch.status === "done" ? (
            build ? (
              <Button size="sm" color="primary" onPress={close}>
                Done
              </Button>
            ) : (
              <>
                <Button variant="light" size="sm" onPress={close}>
                  Close
                </Button>
                <Button
                  size="sm"
                  color="primary"
                  startContent={<IconTerminal2 size={15} />}
                  onPress={goToMachine}
                >
                  Open machine
                </Button>
              </>
            )
          ) : (
            <Button variant="light" size="sm" onPress={close}>
              Dismiss
            </Button>
          )}
        </ModalFooter>
      </ModalContent>
    </Modal>
  );
}
