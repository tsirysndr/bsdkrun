import {
  Button,
  Input,
  Modal,
  ModalBody,
  ModalContent,
  ModalFooter,
  ModalHeader,
} from "@heroui/react";
import { useAtom, useSetAtom } from "jotai";
import { useEffect } from "react";
import { useForm } from "react-hook-form";
import { zodResolver } from "@hookform/resolvers/zod";
import { z } from "zod";
import { IconGitBranch, IconInfoCircle } from "@tabler/icons-react";
import { branchTargetAtom, viewAtom } from "../state/atoms";
import { useBranchSnapshot } from "../lib/queries";
import { useToast } from "../state/toast";
import { shortId } from "../lib/format";

const schema = z.object({
  name: z
    .string()
    .max(40, "Too long")
    .regex(/^[a-zA-Z0-9._-]*$/, "Letters, digits, . _ - only"),
  // One per line / comma-separated, each "[BIND:]HOST:GUEST" — the same shape
  // the Run dialog and `--port` take.
  ports: z.string().max(200),
});

type FormValues = z.infer<typeof schema>;

const splitPorts = (s: string) =>
  s
    .split(/[\s,]+/)
    .map((p) => p.trim())
    .filter(Boolean);

/**
 * Boot a new machine from a snapshot (`bsdkrun branch`).
 *
 * The snapshot is cloned, never booted in place, so the original machine and
 * every other branch of the same snapshot stay independent.
 */
export default function BranchDialog() {
  const [target, setTarget] = useAtom(branchTargetAtom);
  const setView = useSetAtom(viewAtom);
  const branch = useBranchSnapshot();
  const toast = useToast();

  const {
    register,
    handleSubmit,
    reset,
    formState: { errors, isSubmitting },
  } = useForm<FormValues>({
    resolver: zodResolver(schema),
    defaultValues: { name: "", ports: "" },
  });

  useEffect(() => {
    if (target) reset({ name: "", ports: target.ports.join(", ") });
  }, [target, reset]);

  const close = () => setTarget(null);

  const onSubmit = handleSubmit(async (v) => {
    if (!target) return;
    try {
      const id = await branch.mutateAsync({
        snapshot: target.snapshot,
        name: v.name || null,
        ports: splitPorts(v.ports),
      });
      toast(
        "success",
        `Branched into ${shortId(id)}`,
        `From ${target.fromMachine ? "machine" : "snapshot"} ${target.label}`,
      );
      setTarget(null);
      setView("machines");
    } catch (e) {
      toast("error", "Branch failed", String(e));
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
            <IconGitBranch size={18} className="text-violet-300" />
            {target?.fromMachine ? "Branch machine" : "Branch from snapshot"}
          </ModalHeader>
          <ModalBody className="gap-4">
            <p className="text-xs text-foreground-500">
              Boot a new machine from{" "}
              <span className="font-mono text-foreground">{target?.label}</span>.{" "}
              {target?.fromMachine
                ? "Its current state is snapshotted first, then cloned — so the machine itself keeps running, untouched."
                : "The snapshot is cloned, so the machine it came from is untouched and you can branch it again as often as you like."}
            </p>
            <Input
              autoFocus
              size="sm"
              label="Machine name (optional)"
              placeholder="web-experiment"
              variant="bordered"
              isInvalid={!!errors.name}
              errorMessage={errors.name?.message}
              classNames={{ inputWrapper: "border-white/10" }}
              {...register("name")}
            />
            <Input
              size="sm"
              label="Port forwards"
              placeholder="8080:80"
              variant="bordered"
              isInvalid={!!errors.ports}
              errorMessage={errors.ports?.message}
              description="HOST:GUEST, comma-separated. A host port already in use is swapped for a free one."
              classNames={{ inputWrapper: "border-white/10" }}
              {...register("ports")}
            />
            {target?.fromMachine &&
            target?.running &&
            (target.kind === "freebsd" || target.kind === "netbsd") ? (
              <div className="flex items-start gap-2 rounded-lg border border-amber-500/20 bg-amber-500/5 px-3 py-2 text-xs text-amber-300">
                <IconInfoCircle size={15} className="mt-0.5 shrink-0" />
                <span>
                  {target.label} will be <b>powered off</b> to take that snapshot: a
                  mounted BSD filesystem cannot be cloned consistently. Start it again
                  afterwards — the branch is unaffected.
                </span>
              </div>
            ) : null}
            {target?.kind === "freebsd" || target?.kind === "netbsd" ? (
              <div className="flex items-start gap-2 rounded-lg border border-white/10 bg-content2/40 px-3 py-2 text-xs text-foreground-400">
                <IconInfoCircle size={15} className="mt-0.5 shrink-0" />
                <span>
                  A BSD guest boots its whole userland, so give it a few seconds before
                  the branch answers on its ports.
                </span>
              </div>
            ) : null}
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
              startContent={!isSubmitting && <IconGitBranch size={15} />}
            >
              Branch
            </Button>
          </ModalFooter>
        </form>
      </ModalContent>
    </Modal>
  );
}
