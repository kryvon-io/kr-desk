import {
  Cast,
  Eye,
  Loader2,
  MonitorUp,
  MousePointer2,
  Power,
  Router,
  ShieldCheck,
  TriangleAlert,
  Zap,
} from "lucide-react";

import { Button } from "@/components/ui/button";
import { formatId, type ActiveSession, type P2pStatus } from "@/lib/signaling-state";
import { formatCode } from "@/lib/session-auth";
import { cn } from "@/lib/utils";

/** Código de verificação e se a identidade do outro lado foi confirmada. */
export function VerificationLine({ p2p }: { p2p: P2pStatus }) {
  if (!p2p.verificationCode) return null;
  return (
    <span
      className="flex flex-wrap items-center gap-x-1.5 text-muted-foreground"
      title="O mesmo código deve aparecer na outra tela. Se for diferente, encerre a sessão."
    >
      <ShieldCheck
        className={cn("size-3.5", p2p.authenticated ? "text-emerald-500" : "text-muted-foreground")}
      />
      Código de verificação{" "}
      <span className="font-mono font-medium text-foreground">
        {formatCode(p2p.verificationCode)}
      </span>
      {p2p.firstContact && <span>· primeira conexão: confira na outra tela</span>}
    </span>
  );
}

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
  onToggleControl,
}: {
  session: ActiveSession;
  onEnd: (requestId: string) => void;
  /** Host: liga/desliga o controle remoto. */
  onToggleControl?: (requestId: string, allow: boolean) => void;
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
        <span className="pl-6 text-xs">
          <VerificationLine p2p={session.p2p} />
        </span>
        {session.role === "host" && session.p2p.sharing && (
          <span className="flex items-center gap-1.5 pl-6 text-xs font-medium text-emerald-600 dark:text-emerald-400">
            <Cast className="size-3.5" />
            {session.p2p.control
              ? "Sua tela está sendo transmitida e pode ser controlada"
              : "Sua tela está sendo transmitida (somente visualização)"}
          </span>
        )}
      </div>
      <div className="flex shrink-0 flex-col items-stretch gap-1.5">
        {session.role === "host" && onToggleControl && session.p2p.authenticated && (
          <Button
            variant="outline"
            size="sm"
            onClick={() => onToggleControl(session.requestId, !session.p2p.control)}
            title={
              session.p2p.control
                ? "Bloquear mouse e teclado remotos"
                : "Permitir mouse e teclado remotos"
            }
          >
            {session.p2p.control ? <Eye /> : <MousePointer2 />}
            {session.p2p.control ? "Só visualizar" : "Permitir controle"}
          </Button>
        )}
        <Button variant="outline" size="sm" onClick={() => onEnd(session.requestId)}>
          <Power /> Encerrar
        </Button>
      </div>
    </div>
  );
}
