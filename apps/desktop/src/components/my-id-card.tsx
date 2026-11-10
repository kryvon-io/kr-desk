import { Check, Copy } from "lucide-react";
import { useState } from "react";

import { Button } from "@/components/ui/button";
import { formatId, type ConnectionStatus } from "@/lib/signaling-state";
import { cn } from "@/lib/utils";

/** Cartão com o ID deste computador, para passar a quem vai acessar. */
interface Props {
  status: ConnectionStatus;
  deviceId: string | null;
  deviceName: string;
}

export function MyIdCard({ status, deviceId: id, deviceName }: Props) {
  const [copied, setCopied] = useState(false);
  const online = status.state === "online";

  async function copy() {
    if (!id) return;
    await navigator.clipboard.writeText(id);
    setCopied(true);
    setTimeout(() => setCopied(false), 1500);
  }

  return (
    <section className="rounded-xl border bg-card p-6 shadow-sm">
      <div className="flex items-baseline justify-between gap-2">
        <h2 className="text-sm text-muted-foreground">ID deste computador</h2>
        {deviceName && (
          <span className="truncate text-xs text-muted-foreground" title={deviceName}>
            {deviceName}
          </span>
        )}
      </div>
      <div className="mt-2 flex items-center gap-2">
        <p
          className={cn(
            "flex-1 font-mono text-3xl font-semibold tracking-wider tabular-nums",
            !online && "opacity-50",
          )}
          aria-live="polite"
          title={online ? undefined : "Sem conexão: o ID volta a funcionar quando reconectar"}
        >
          {id ? formatId(id) : <span className="text-muted-foreground">— — —</span>}
        </p>
        <Button
          variant="outline"
          size="icon"
          onClick={copy}
          disabled={!id}
          aria-label="Copiar ID"
          title="Copiar ID"
        >
          {copied ? <Check /> : <Copy />}
        </Button>
      </div>
      <p className="mt-2 text-xs text-muted-foreground">
        Passe este número para quem vai acessar este computador. Nenhum acesso acontece sem a sua
        autorização.
      </p>
    </section>
  );
}
