import {
  Button,
  Input,
  Modal,
  ModalBody,
  ModalContent,
  ModalFooter,
  ModalHeader,
} from "@heroui/react";
import { useAtom } from "jotai";
import { useEffect } from "react";
import { Controller, useForm } from "react-hook-form";
import { zodResolver } from "@hookform/resolvers/zod";
import { z } from "zod";
import { IconCpu, IconInfoCircle } from "@tabler/icons-react";
import { editResourcesAtom } from "../state/atoms";
import { useUpdateMachine } from "../lib/queries";
import { useToast } from "../state/toast";

const schema = z.object({
  cpus: z.coerce
    .number({ invalid_type_error: "Enter a number" })
    .int()
    .min(1, "At least 1")
    .max(64, "Too many"),
  mem: z.coerce
    .number({ invalid_type_error: "Enter a number" })
    .int()
    .min(128, "Min 128 MiB"),
});

type FormValues = z.infer<typeof schema>;

/**
 * Edit a machine's vCPU / memory. libkrun fixes VM resources at boot, so the new
 * values apply on the next start — surfaced clearly for a running machine.
 */
export default function EditResourcesDialog() {
  const [target, setTarget] = useAtom(editResourcesAtom);
  const update = useUpdateMachine();
  const toast = useToast();

  const {
    control,
    handleSubmit,
    reset,
    formState: { errors, isSubmitting },
  } = useForm<FormValues>({
    resolver: zodResolver(schema),
    defaultValues: { cpus: 1, mem: 512 },
  });

  useEffect(() => {
    if (target) reset({ cpus: target.cpus, mem: target.mem });
  }, [target, reset]);

  const close = () => setTarget(null);

  const onSubmit = handleSubmit(async (v) => {
    if (!target) return;
    try {
      await update.mutateAsync({ id: target.id, cpus: v.cpus, mem: v.mem });
      toast(
        "success",
        "Resources updated",
        target.running
          ? `${v.cpus} vCPU · ${v.mem} MiB — restart ${target.label} to apply`
          : `${v.cpus} vCPU · ${v.mem} MiB`,
      );
      setTarget(null);
    } catch (e) {
      toast("error", "Couldn't update resources", String(e));
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
            <IconCpu size={18} className="text-primary" />
            Edit resources
          </ModalHeader>
          <ModalBody className="gap-4">
            <div className="grid grid-cols-2 gap-3">
              <Controller
                control={control}
                name="cpus"
                render={({ field }) => (
                  <Input
                    type="number"
                    label="vCPUs"
                    labelPlacement="outside"
                    value={String(field.value)}
                    onValueChange={field.onChange}
                    min={1}
                    max={64}
                    variant="bordered"
                    isInvalid={!!errors.cpus}
                    errorMessage={errors.cpus?.message}
                    classNames={{ inputWrapper: "border-white/10" }}
                  />
                )}
              />
              <Controller
                control={control}
                name="mem"
                render={({ field }) => (
                  <Input
                    type="number"
                    label="Memory (MiB)"
                    labelPlacement="outside"
                    value={String(field.value)}
                    onValueChange={field.onChange}
                    min={128}
                    step={128}
                    variant="bordered"
                    isInvalid={!!errors.mem}
                    errorMessage={errors.mem?.message}
                    classNames={{ inputWrapper: "border-white/10" }}
                  />
                )}
              />
            </div>
            {target?.running && (
              <div className="flex items-start gap-2 rounded-lg border border-amber-500/20 bg-amber-500/5 px-3 py-2 text-xs text-amber-300">
                <IconInfoCircle size={15} className="mt-0.5 shrink-0" />
                <span>
                  {target.label} is running. New resources take effect the next
                  time it's started (stop &amp; start to apply).
                </span>
              </div>
            )}
          </ModalBody>
          <ModalFooter>
            <Button variant="light" size="sm" isDisabled={isSubmitting} onPress={close}>
              Cancel
            </Button>
            <Button type="submit" size="sm" color="primary" isLoading={isSubmitting}>
              Save
            </Button>
          </ModalFooter>
        </form>
      </ModalContent>
    </Modal>
  );
}
