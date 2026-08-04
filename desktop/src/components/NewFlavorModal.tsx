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
import { IconApps } from "@tabler/icons-react";
import { newFlavorOpenAtom } from "../state/atoms";
import { useCreateFlavor } from "../lib/queries";
import { useBuildFlavor } from "../hooks/useLaunchFlavor";
import { useToast } from "../state/toast";

const CATEGORIES = [
  "language",
  "runtime",
  "service",
  "web",
  "ai",
  "os",
  "custom",
];

// Split a textarea into trimmed non-empty lines.
const lines = (s: string) =>
  s
    .split("\n")
    .map((l) => l.trim())
    .filter(Boolean);

const schema = z.object({
  name: z
    .string()
    .min(1, "A name is required")
    .max(40, "Too long")
    .regex(/^[a-zA-Z0-9._-]+$/, "Letters, digits, . _ - only"),
  base: z.string().min(1, "A base image is required"),
  category: z.string(),
  description: z.string().max(120, "Keep it under 120 chars"),
  ports: z
    .string()
    .refine(
      (s) => lines(s).every((l) => /^\d+:\d+$/.test(l)),
      "Each line must be HOST:GUEST (numbers)",
    ),
  env: z.string(),
  nix: z.string(),
  provision: z.string(),
});

type FormValues = z.infer<typeof schema>;

const DEFAULTS: FormValues = {
  name: "",
  base: "",
  category: "custom",
  description: "",
  ports: "",
  env: "",
  nix: "",
  provision: "",
};

/**
 * Define a custom flavor. Persists to the user's `flavors.toml` (via
 * `bsdkrun flavor add`) so it appears alongside the catalog and can be launched
 * like any other flavor.
 */
export default function NewFlavorModal() {
  const [open, setOpen] = useAtom(newFlavorOpenAtom);
  const create = useCreateFlavor();
  const buildFlavor = useBuildFlavor();
  const toast = useToast();

  const {
    register,
    handleSubmit,
    reset,
    formState: { errors, isSubmitting },
  } = useForm<FormValues>({ resolver: zodResolver(schema), defaultValues: DEFAULTS });

  useEffect(() => {
    if (open) reset(DEFAULTS);
  }, [open, reset]);

  const onSubmit = handleSubmit(async (v) => {
    const nix = lines(v.nix);
    const provision = lines(v.provision);
    try {
      await create.mutateAsync({
        name: v.name,
        base: v.base.trim(),
        category: v.category,
        description: v.description.trim(),
        ports: lines(v.ports),
        env: lines(v.env),
        nix,
        provision,
      });
      setOpen(false);
      // If it has provisioning, build it right away and stream the logs so the
      // first real launch is instant; otherwise it's ready to go as-is.
      if (nix.length > 0 || provision.length > 0) {
        toast("success", `Flavor “${v.name}” saved — building…`);
        buildFlavor(v.name);
      } else {
        toast("success", `Flavor “${v.name}” saved`, "Find it under Flavors");
      }
    } catch (e) {
      toast("error", "Couldn't save flavor", String(e));
    }
  });

  return (
    <Modal
      isOpen={open}
      onClose={() => {
        if (!isSubmitting) setOpen(false);
      }}
      size="xl"
      scrollBehavior="inside"
      backdrop="opaque"
      shouldBlockScroll={false}
      classNames={{ base: "border border-white/10 bg-content1" }}
    >
      <ModalContent>
        <form onSubmit={onSubmit}>
          <ModalHeader className="flex items-center gap-2 text-base">
            <IconApps size={18} className="text-primary" />
            New flavor
          </ModalHeader>
          <ModalBody className="gap-4">
            <div className="grid grid-cols-2 gap-3">
              <Input
                size="sm"
                label="Name"
                placeholder="my-stack"
                variant="bordered"
                isInvalid={!!errors.name}
                errorMessage={errors.name?.message}
                classNames={{ inputWrapper: "border-white/10" }}
                {...register("name")}
              />
              <div className="flex flex-col gap-1">
                <label className="text-xs text-foreground-500">Category</label>
                {/* Native select — HeroUI's Select can freeze the WKWebview. */}
                <select
                  className="h-10 rounded-medium border border-white/10 bg-content2/40 px-3 text-sm text-foreground outline-none"
                  {...register("category")}
                >
                  {CATEGORIES.map((c) => (
                    <option key={c} value={c} className="bg-content1 text-foreground">
                      {c}
                    </option>
                  ))}
                </select>
              </div>
            </div>
            <Input
              size="sm"
              label="Base image"
              placeholder="node:22  ·  or  freebsd / netbsd"
              variant="bordered"
              isInvalid={!!errors.base}
              errorMessage={errors.base?.message}
              classNames={{ inputWrapper: "border-white/10", input: "font-mono text-xs" }}
              {...register("base")}
            />
            <Input
              size="sm"
              label="Description"
              placeholder="What this environment is for"
              variant="bordered"
              isInvalid={!!errors.description}
              errorMessage={errors.description?.message}
              classNames={{ inputWrapper: "border-white/10" }}
              {...register("description")}
            />
            <div className="grid grid-cols-2 gap-3">
              <Textarea
                size="sm"
                label="Ports (one HOST:GUEST per line)"
                placeholder={"3000:3000\n5432:5432"}
                minRows={2}
                variant="bordered"
                isInvalid={!!errors.ports}
                errorMessage={errors.ports?.message}
                classNames={{ inputWrapper: "border-white/10", input: "font-mono text-xs" }}
                {...register("ports")}
              />
              <Textarea
                size="sm"
                label="Env (one K=V per line)"
                placeholder={"NODE_ENV=development"}
                minRows={2}
                variant="bordered"
                classNames={{ inputWrapper: "border-white/10", input: "font-mono text-xs" }}
                {...register("env")}
              />
            </div>
            <Textarea
              size="sm"
              label="Nix packages (one per line — OCI base only)"
              placeholder={"ripgrep\nfd"}
              minRows={2}
              variant="bordered"
              classNames={{ inputWrapper: "border-white/10", input: "font-mono text-xs" }}
              {...register("nix")}
            />
            <Textarea
              size="sm"
              label="Provision (one shell command per line, run in order)"
              placeholder={"apt-get update && apt-get install -y git\nnpm install -g pnpm"}
              minRows={3}
              variant="bordered"
              classNames={{ inputWrapper: "border-white/10", input: "font-mono text-xs" }}
              {...register("provision")}
            />
            <p className="text-xs text-foreground-500">
              Provisioned flavors are built once and cached — the first launch
              runs these steps, later launches clone the result instantly.
            </p>
          </ModalBody>
          <ModalFooter>
            <Button
              variant="light"
              size="sm"
              isDisabled={isSubmitting}
              onPress={() => setOpen(false)}
            >
              Cancel
            </Button>
            <Button type="submit" size="sm" color="primary" isLoading={isSubmitting}>
              Save flavor
            </Button>
          </ModalFooter>
        </form>
      </ModalContent>
    </Modal>
  );
}
