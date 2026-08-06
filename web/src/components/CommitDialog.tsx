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
import { useAtom } from "jotai";
import { useEffect } from "react";
import { useForm } from "react-hook-form";
import { zodResolver } from "@hookform/resolvers/zod";
import { z } from "zod";
import { IconCamera, IconInfoCircle } from "@tabler/icons-react";
import { commitTargetAtom, viewAtom } from "../state/atoms";
import { useCommitMachine } from "../lib/queries";
import { useToast } from "../state/toast";
import { useSetAtom } from "jotai";

// A snapshot name doubles as a directory + flavor key, so keep it filesystem-safe.
const schema = z.object({
  name: z
    .string()
    .min(1, "A name is required")
    .max(40, "Too long")
    .regex(/^[a-zA-Z0-9._-]+$/, "Letters, digits, . _ - only"),
  description: z.string().max(120, "Keep it under 120 chars"),
});

type FormValues = z.infer<typeof schema>;

/**
 * Snapshot a machine's current state into a reusable flavor (`bsdkrun commit`).
 * Opened from a machine row / detail drawer via `commitTargetAtom`.
 */
export default function CommitDialog() {
  const [target, setTarget] = useAtom(commitTargetAtom);
  const setView = useSetAtom(viewAtom);
  const commit = useCommitMachine();
  const toast = useToast();
  const isBsd = target ? target.kind !== "linux" : false;

  const {
    register,
    handleSubmit,
    reset,
    formState: { errors, isSubmitting },
  } = useForm<FormValues>({
    resolver: zodResolver(schema),
    defaultValues: { name: "", description: "" },
  });

  // Reset the form whenever a new machine is targeted.
  useEffect(() => {
    if (target) reset({ name: "", description: "" });
  }, [target, reset]);

  const close = () => setTarget(null);

  const onSubmit = handleSubmit(async (v) => {
    if (!target) return;
    try {
      await commit.mutateAsync({
        id: target.id,
        name: v.name,
        description: v.description,
      });
      toast("success", `Snapshot “${v.name}” saved`, "Find it under Flavors → Your snapshots");
      setTarget(null);
      setView("flavors");
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
            <IconCamera size={18} className="text-rose-300" />
            Snapshot machine
          </ModalHeader>
          <ModalBody className="gap-4">
            <p className="text-xs text-foreground-500">
              Save the current state of{" "}
              <span className="font-medium text-foreground">{target?.label}</span> as a
              reusable flavor. Boot fresh machines from it any time — packages, files and
              all.
            </p>
            <Input
              autoFocus
              size="sm"
              label="Flavor name"
              placeholder="my-freebsd-dev"
              variant="bordered"
              isInvalid={!!errors.name}
              errorMessage={errors.name?.message}
              classNames={{ inputWrapper: "border-white/10" }}
              {...register("name")}
            />
            <Textarea
              size="sm"
              label="Description (optional)"
              placeholder="FreeBSD 15 with my toolchain preinstalled"
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
                  {target.label} will be <b>powered off</b> to capture a clean,
                  consistent disk image (a live BSD filesystem can't be snapshotted
                  safely). Start it again afterwards to resume.
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
              Save snapshot
            </Button>
          </ModalFooter>
        </form>
      </ModalContent>
    </Modal>
  );
}
