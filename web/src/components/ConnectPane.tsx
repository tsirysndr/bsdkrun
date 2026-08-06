import { useState } from "react";
import { Button, Card, CardBody, Input } from "@heroui/react";
import {
  IconAlertTriangle,
  IconPlugConnected,
  IconServerBolt,
} from "@tabler/icons-react";
import { DEFAULT_URL, normalizeUrl, setConnection } from "../lib/connection";
import { gql } from "../lib/graphql";

/**
 * First run: the app has no idea where its daemon is.
 *
 * Unlike the desktop app — which finds a `bsdkrun` binary on the machine it is
 * already running on — a web build is served from anywhere and has to be told
 * which daemon to drive. The connection is verified here rather than saved
 * blindly, so a wrong URL or token is a clear message on this screen instead of
 * every panel in the app failing at once.
 */
export default function ConnectPane({ onConnected }: { onConnected: () => void }) {
  const [url, setUrl] = useState(DEFAULT_URL);
  const [token, setToken] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const connect = async () => {
    setBusy(true);
    setError(null);
    // Saved before the probe because the transport reads its settings from
    // here; a failure below clears it again.
    setConnection({ url, token });
    try {
      await gql(`{ info { cliVersion } }`);
      onConnected();
    } catch (e) {
      setError((e as Error).message);
      setConnection({ url: "", token: "" });
    } finally {
      setBusy(false);
    }
  };

  const canSubmit = url.trim() !== "" && token.trim() !== "" && !busy;

  return (
    <div className="flex h-screen w-screen items-center justify-center bg-background p-6">
      <Card className="w-full max-w-lg border border-white/10 bg-content1">
        <CardBody className="gap-5 p-7">
          <div className="flex items-center gap-3">
            <div className="rounded-xl bg-primary/10 p-2.5">
              <IconServerBolt size={22} className="text-primary" />
            </div>
            <div>
              <h1 className="text-lg font-semibold">Connect to bsdkrun</h1>
              <p className="text-xs text-foreground-500">
                Point this UI at a running <code className="font-mono">bsdkrund</code>.
              </p>
            </div>
          </div>

          <div>
            <label className="mb-1.5 block text-sm font-medium">GraphQL API URL</label>
            <Input
              value={url}
              onValueChange={setUrl}
              placeholder={DEFAULT_URL}
              variant="bordered"
              autoFocus
              classNames={{ input: "font-mono text-xs" }}
            />
            <p className="mt-1.5 text-xs text-foreground-500">
              A bare host like <span className="font-mono">localhost:50052</span> works —{" "}
              <span className="font-mono">/graphql</span> is added for you.
            </p>
          </div>

          <div>
            <label className="mb-1.5 block text-sm font-medium">Access token</label>
            <Input
              value={token}
              onValueChange={setToken}
              placeholder="the token bsdkrund printed on startup"
              variant="bordered"
              type="password"
              classNames={{ input: "font-mono text-xs" }}
              onKeyDown={(e) => {
                if (e.key === "Enter" && canSubmit) connect();
              }}
            />
          </div>

          {error && (
            <div className="flex items-start gap-2.5 rounded-xl border border-danger/20 bg-danger/5 p-3">
              <IconAlertTriangle size={18} className="mt-0.5 shrink-0 text-danger" />
              <div className="min-w-0 text-xs text-foreground-500">
                <div className="font-medium text-foreground">Could not connect</div>
                <div className="mt-0.5 break-words">{error}</div>
              </div>
            </div>
          )}

          <Button
            color="primary"
            isLoading={busy}
            isDisabled={!canSubmit}
            startContent={!busy ? <IconPlugConnected size={16} /> : undefined}
            onPress={connect}
          >
            Connect
          </Button>

          <div className="rounded-xl border border-white/10 bg-content2/40 p-3">
            <div className="mb-1 text-xs font-medium text-foreground-500">
              Start a daemon on the machine that runs the VMs:
            </div>
            <code className="block break-all font-mono text-[11px] text-foreground-400">
              bsdkrund --graphql-bind 0.0.0.0:50052
            </code>
            <div className="mt-1.5 text-[11px] text-foreground-500">
              It prints an access token on startup. The URL is normalized to{" "}
              <span className="font-mono">{normalizeUrl(url) || "…"}</span>.
            </div>
          </div>
        </CardBody>
      </Card>
    </div>
  );
}
