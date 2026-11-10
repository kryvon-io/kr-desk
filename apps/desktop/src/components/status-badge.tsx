import { useEffect, useState } from "react";

import type { ConnectionStatus } from "@/lib/signaling-state";
import { cn } from "@/lib/utils";

/** Contagem regressiva até a próxima tentativa (remontado a cada falha via `key`). */
function RetryCountdown({ retryInMs }: { retryInMs: number }) {
  const [deadline] = useState(() => Date.now() + retryInMs);
  const [now, setNow] = useState(() => Date.now());
  useEffect(() => {
    const t = setInterval(() => setNow(Date.now()), 500);
    return () => clearInterval(t);
  }, []);
  const secs = Math.max(0, Math.ceil((deadline - now) / 1000));
  return <>{secs > 0 ? `Sem conexão · nova tentativa em ${secs}s` : "Sem conexão · tentando…"}</>;
}

/** Indicador do estado da conexão com o servidor. */
export function StatusBadge({ status }: { status: ConnectionStatus }) {
  const dot =
    status.state === "online"
      ? "bg-emerald-500"
      : status.state === "connecting"
        ? "animate-pulse bg-amber-400"
        : "bg-red-500";

  return (
    <div
      className="flex items-center gap-2 rounded-full border px-3 py-1 text-xs text-muted-foreground"
      title={status.state === "offline" ? status.reason : undefined}
      role="status"
    >
      <span className={cn("size-2 rounded-full", dot)} aria-hidden />
      {status.state === "online" && "Pronto para conexões"}
      {status.state === "connecting" && "Conectando…"}
      {status.state === "offline" && (
        <RetryCountdown key={`${status.reason}:${status.retryInMs}`} retryInMs={status.retryInMs} />
      )}
    </div>
  );
}
