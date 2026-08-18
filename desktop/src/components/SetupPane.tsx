import { useState } from "react";
import { Button, Input, Snippet } from "@heroui/react";
import {
  IconKey,
  IconNetwork,
  IconRefresh,
  IconDownload,
} from "@tabler/icons-react";
import { api } from "../lib/api";
import { useUiTheme } from "../lib/theme";
import { useToast } from "../state/toast";

function Output({ text }: { text: string }) {
  const ui = useUiTheme();
  if (!text) return null;
  return (
    <pre className={`mt-2 max-h-56 overflow-auto whitespace-pre-wrap rounded-lg border border-white/10 ${ui.surface} p-3 font-mono text-[13px] leading-relaxed text-foreground-300`}>
      {text}
    </pre>
  );
}

/** In-guest setup actions (agent-backed): key-based SSH and Tailscale. Requires
 *  the machine to be running with networking + its agent up. */
export default function SetupPane({ machineId }: { machineId: string }) {
  const toast = useToast();
  const [authkey, setAuthkey] = useState("");
  const [busy, setBusy] = useState<string | null>(null);
  const [sshOut, setSshOut] = useState("");
  const [tsOut, setTsOut] = useState("");
  const [agentOut, setAgentOut] = useState("");

  const run = async (
    label: string,
    fn: () => Promise<string>,
    set: (s: string) => void,
  ) => {
    setBusy(label);
    try {
      // The backend returns the command's real output regardless of exit code
      // (a non-zero `status` just means "not set up / not running"), so the
      // panel shows the actual state instead of a generic error.
      const out = await fn();
      set(out.trim() || "done");
      toast("info", label);
    } catch (e) {
      // Only reached on spawn/timeout — a genuine invocation failure.
      set(String(e));
      toast("error", `${label} failed`, String(e));
    } finally {
      setBusy(null);
    }
  };

  return (
    <div className="flex flex-col gap-6 p-5">
      {/* Update agent */}
      <section className="rounded-xl border border-amber-500/20 bg-amber-500/5 p-4">
        <div className="mb-1 flex items-center gap-2 text-sm font-medium">
          <IconDownload size={16} className="text-amber-400" /> Guest agent
        </div>
        <p className="mb-3 text-xs text-foreground-500">
          If SSH/Tailscale setup fails with{" "}
          <span className="font-mono">Address already in use</span>, the image
          shipped an older agent. This installs the current one in the running
          guest (no restart needed).
        </p>
        <Button
          size="sm"
          variant="flat"
          startContent={!(busy === "Update agent") && <IconDownload size={14} />}
          isLoading={busy === "Update agent"}
          onPress={() =>
            run("Update agent", () => api.updateAgent(machineId), setAgentOut)
          }
        >
          Update guest agent
        </Button>
        <Output text={agentOut} />
      </section>

      {/* SSH */}
      <section>
        <div className="mb-1 flex items-center gap-2 text-sm font-medium">
          <IconKey size={16} className="text-primary" /> SSH access
        </div>
        <p className="mb-3 text-xs text-foreground-500">
          Installs your local <span className="font-mono">~/.ssh/id_*.pub</span>{" "}
          into the guest and enables key-based login.
        </p>
        <div className="flex gap-2">
          <Button
            size="sm"
            color="primary"
            variant="flat"
            isLoading={busy === "SSH setup"}
            onPress={() =>
              run("SSH setup", () => api.sshAction(machineId, ["setup"]), setSshOut)
            }
          >
            Set up SSH
          </Button>
          <Button
            size="sm"
            variant="flat"
            startContent={<IconRefresh size={14} />}
            isLoading={busy === "SSH status"}
            onPress={() =>
              run("SSH status", () => api.sshAction(machineId, ["status"]), setSshOut)
            }
          >
            Status
          </Button>
        </div>
        <Output text={sshOut} />
      </section>

      {/* Tailscale */}
      <section>
        <div className="mb-1 flex items-center gap-2 text-sm font-medium">
          <IconNetwork size={16} className="text-primary" /> Tailscale
        </div>
        <p className="mb-3 text-xs text-foreground-500">
          Join the guest to your tailnet. Provide an auth key for unattended
          setup (from the Tailscale admin console).
        </p>
        <Input
          size="sm"
          variant="bordered"
          placeholder="tskey-auth-… (optional)"
          value={authkey}
          onValueChange={setAuthkey}
          type="password"
          classNames={{ input: "font-mono text-xs" }}
          className="mb-2 max-w-md"
        />
        <div className="flex flex-wrap gap-2">
          <Button
            size="sm"
            color="primary"
            variant="flat"
            isLoading={busy === "Tailscale setup"}
            onPress={() =>
              run(
                "Tailscale setup",
                () =>
                  api.tailscaleAction(
                    machineId,
                    authkey.trim()
                      ? ["setup", "--authkey", authkey.trim()]
                      : ["setup"],
                  ),
                setTsOut,
              )
            }
          >
            Set up Tailscale
          </Button>
          <Button
            size="sm"
            variant="flat"
            isLoading={busy === "Tailscale install"}
            onPress={() =>
              run(
                "Tailscale install",
                () => api.tailscaleAction(machineId, ["install"]),
                setTsOut,
              )
            }
          >
            Install
          </Button>
          <Button
            size="sm"
            variant="flat"
            startContent={<IconRefresh size={14} />}
            isLoading={busy === "Tailscale status"}
            onPress={() =>
              run(
                "Tailscale status",
                () => api.tailscaleAction(machineId, ["status"]),
                setTsOut,
              )
            }
          >
            Status
          </Button>
        </div>
        <Output text={tsOut} />
      </section>

      <p className="text-[11px] text-foreground-600">
        These run inside the guest via its agent — the machine must be running
        with networking up.
      </p>
      <Snippet
        size="sm"
        variant="bordered"
        symbol=""
        classNames={{ base: "border-white/10 bg-content2/40", pre: "text-[11px]" }}
      >
        {`bsdkrun ssh ${machineId.slice(0, 12)} setup`}
      </Snippet>
    </div>
  );
}
