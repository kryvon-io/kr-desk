import { MonitorUp, Power } from "lucide-react";

import { Button } from "@/components/ui/button";
import { formatId, type ActiveSession } from "@/lib/signaling-state";

/** Faixa sempre visível enquanto há uma sessão autorizada, com botão de encerrar. */
export function SessionBanner({
  session,
  onEnd,
}: {
  session: ActiveSession;
  onEnd: (requestId: string) => void;
}) {
  const text =
    session.role === "host"
      ? `Este computador autorizou o acesso de ${formatId(session.peerId)}`
      : `Sessão autorizada com ${formatId(session.peerId)}`;
  return (
    <div
      className="flex w-full max-w-md items-center justify-between gap-3 rounded-xl border border-emerald-500/40 bg-emerald-500/10 px-4 py-3 text-sm"
      role="status"
    >
      <span className="flex items-center gap-2">
        <MonitorUp className="size-4 text-emerald-500" />
        {text}
      </span>
      <Button variant="outline" size="sm" onClick={() => onEnd(session.requestId)}>
        <Power /> Encerrar
      </Button>
    </div>
  );
}
