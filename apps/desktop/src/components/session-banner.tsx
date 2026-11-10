import { Cast, Loader2, MonitorUp, Power, Router, TriangleAlert, Zap } from "lucide-react";

import { Button } from "@/components/ui/button";
import { formatId, type ActiveSession, type P2pStatus } from "@/lib/signaling-state";
import { cn } from "@/lib/utils";

/** Descrição curta da conexão ponto a ponto. */
function P2pLine({ p2p }: { p2p: P2pStatus }) {
  const rtt = p2p.rttMs !== null ? ` · ${p2p.rttMs} ms` : "";
  if (p2p.state === "failed") {
    return (
      <span className="flex items-center gap-1.5 text-destructive">
        <TriangleAlert className="size-3.5" /> Não foi possível conectar os dois computadores
      </span>
    );
  }
  if (p2p.state === "disconnected") {
    return (
      <span className="flex items-center gap-1.5 text-amber-500">
        <Loader2 className="size-3.5 animate-spin" /> Conexão instável, tentando recuperar…
      </span>
    );
  }
  if (p2p.state !== "connected") {
    return (
      <span className="flex items-center gap-1.5 text-muted-foreground">
        <Loader2 className="size-3.5 animate-spin" /> Estabelecendo conexão direta…
      </span>
    );
  }
  if (p2p.route?.relayed) {
    return (
      <span className="flex items-center gap-1.5 text-muted-foreground" title="Via servidor TURN">
        <Router className="size-3.5" /> Conectado via servidor{rtt}
      </span>
    );
  }
  return (
    <span
      className="flex items-center gap-1.5 text-muted-foreground"
      title={
        p2p.route ? `${p2p.route.local} → ${p2p.route.remote} (${p2p.route.protocol})` : undefined
      }
    >
      <Zap className="size-3.5 text-emerald-500" /> Conexão direta{rtt}
    </span>
  );
}

/** Faixa sempre visível enquanto há uma sessão autorizada, com botão de encerrar. */
export function SessionBanner({
  session,
  onEnd,
}: {
  session: ActiveSession;
  onEnd: (requestId: string) => void;
}) {
  const peer = <span className="whitespace-nowrap font-mono">{formatId(session.peerId)}</span>;
  const text =
    session.role === "host" ? (
      <>Este computador autorizou o acesso de {peer}</>
    ) : (
      <>Sessão autorizada com {peer}</>
    );
  const failed = session.p2p.state === "failed";
  return (
    <div
      className={cn(
        "flex w-full max-w-md items-center justify-between gap-3 rounded-xl border px-4 py-3 text-sm",
        failed
          ? "border-destructive/40 bg-destructive/10"
          : "border-emerald-500/40 bg-emerald-500/10",
      )}
      role="status"
    >
      <div className="flex min-w-0 flex-col gap-1">
        <span className="flex items-center gap-2">
          <MonitorUp className="size-4 shrink-0 text-emerald-500" />
          {text}
        </span>
        <span className="pl-6 text-xs" aria-live="polite">
          <P2pLine p2p={session.p2p} />
        </span>
        {session.role === "host" && session.p2p.sharing && (
          <span className="flex items-center gap-1.5 pl-6 text-xs font-medium text-emerald-600 dark:text-emerald-400">
            <Cast className="size-3.5" /> Sua tela está sendo transmitida
          </span>
        )}
      </div>
      <Button variant="outline" size="sm" onClick={() => onEnd(session.requestId)}>
        <Power /> Encerrar
      </Button>
    </div>
  );
}
