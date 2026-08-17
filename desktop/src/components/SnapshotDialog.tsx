import {
  Button,
  Input,
  Modal,
  ModalBody,
  ModalContent,
  ModalFooter,
  ModalHeader,
  Textarea,
} from "@heroui/react";
import { useAtom, useSetAtom } from "jotai";
import { useEffect } from "react";
import { useForm } from "react-hook-form";
import { zodResolver } from "@hookform/resolvers/zod";
import { z } from "zod";
import { IconCamera, IconInfoCircle } from "@tabler/icons-react";
import { snapshotTargetAtom, viewAtom } from "../state/atoms";
import { useSnapshotMachine } from "../lib/queries";
import { useToast } from "../state/toast";

// The name is a DB key and shows up in `branch <name>`, so keep it shell- and
// filesystem-safe. Empty is allowed: the engine then names it `<machine>-<n>`.
const schema = z.object({
  name: z
    .string()
    .max(40, "Too long")
    .regex(/^[a-zA-Z0-9._-]*$/, "Letters, digits, . _ - only"),
  description: z.string().max(120, "Keep it under 120 chars"),
});

type FormValues = z.infer<typeof schema>;

/**
 * Take a snapshot of a machine's disk state (`bsdkrun snapshot`).
 *
 * Opened from a machine row / detail drawer via `snapshotTargetAtom`. Unlike
 * `commit`, this does not create a flavor: it captures *this machine's* state,
 * which can then be branched into a new machine or restored over the original.
 */
export default function SnapshotDialog() {
  const [target, setTarget] = useAtom(snapshotTargetAtom);
  const setView = useSetAtom(viewAtom);
  const snapshot = useSnapshotMachine();
  const toast = useToast();
  const isBsd = target ? target.kind === "freebsd" || target.kind === "netbsd" : false;

  const {
    register,
    handleSubmit,
    reset,
    formState: { errors, isSubmitting },
  } = useForm<FormValues>({
    resolver: zodResolver(schema),
    defaultValues: { name: "", description: "" },
  });

  useEffect(() => {
    if (target) reset({ name: "", description: "" });
  }, [target, reset]);

  const close = () => setTarget(null);

  const onSubmit = handleSubmit(async (v) => {
    if (!target) return;
    try {
      const name = await snapshot.mutateAsync({
        id: target.id,
        name: v.name || null,
        description: v.description,
      });
      toast("success", `Snapshot “${name}” saved`, "Branch or restore it from Snapshots");
      setTarget(null);
      setView("snapshots");
    } catch (e) {
      toast("error", "Snapshot failed", String(e));
    }
  });

  return (
    <Modal
      isOpen={target !== null}
      onClose={() => {
        if (!isSubmitting) close();
      }}
      size="md"
      backdrop="opaque"
      shouldBlockScroll={false}
      classNames={{ base: "border border-white/10 bg-content1" }}
    >
      <ModalContent>
        <form onSubmit={onSubmit}>
          <ModalHeader className="flex items-center gap-2 text-base">
            <IconCamera size={18} className="text-sky-300" />
            Take a snapshot
          </ModalHeader>
          <ModalBody className="gap-4">
            <p className="text-xs text-foreground-500">
              Capture the disk state of{" "}
              <span className="font-medium text-foreground">{target?.label}</span> right
              now. It is a copy-on-write clone — instant, and free until the two sides
              diverge — so you can branch it into a throwaway machine or roll this one
              back to it later.
            </p>
            <Input
              autoFocus
              size="sm"
              label="Name (optional)"
              placeholder="before-upgrade"
              variant="bordered"
              isInvalid={!!errors.name}
              errorMessage={errors.name?.message}
              description="Left empty, it is named after the machine."
              classNames={{ inputWrapper: "border-white/10" }}
              {...register("name")}
            />
            <Textarea
              size="sm"
              label="Description (optional)"
              placeholder="Clean install, before the 15.1 upgrade"
              minRows={2}
              variant="bordered"
              isInvalid={!!errors.description}
              errorMessage={errors.description?.message}
              classNames={{ inputWrapper: "border-white/10" }}
              {...register("description")}
            />
            {isBsd && target?.running && (
              <div className="flex items-start gap-2 rounded-lg border border-amber-500/20 bg-amber-500/5 px-3 py-2 text-xs text-amber-300">
                <IconInfoCircle size={15} className="mt-0.5 shrink-0" />
                <span>
                  {target.label} will be <b>powered off</b> first: a mounted BSD
                  filesystem cannot be cloned consistently. Start it again afterwards.
                </span>
              </div>
            )}
          </ModalBody>
          <ModalFooter>
            <Button variant="light" size="sm" isDisabled={isSubmitting} onPress={close}>
              Cancel
            </Button>
            <Button
              type="submit"
              size="sm"
              color="primary"
              isLoading={isSubmitting}
              startContent={!isSubmitting && <IconCamera size={15} />}
            >
              Take snapshot
            </Button>
          </ModalFooter>
        </form>
      </ModalContent>
    </Modal>
  );
}
