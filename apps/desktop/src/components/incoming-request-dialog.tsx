import { ShieldAlert } from "lucide-react";
import { useEffect, useState } from "react";

import { Button } from "@/components/ui/button";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { formatId, type IncomingRequest } from "@/lib/signaling-state";

/** Tempo para decidir antes de recusar automaticamente (o servidor expira em 45 s). */
export const DECISION_SECONDS = 30;

interface Props {
  request: IncomingRequest | undefined;
  onAnswer: (requestId: string, accept: boolean, allowControl?: boolean) => void;
}

/** Pede autorização ao usuário deste computador. Sem resposta, recusa sozinho. */
export function IncomingRequestDialog({ request, onAnswer }: Props) {
  const [now, setNow] = useState(() => Date.now());
  const [allowControl, setAllowControl] = useState(true);

  useEffect(() => {
    if (!request) return;
    const t = setInterval(() => setNow(Date.now()), 250);
    return () => clearInterval(t);
  }, [request]);

  const remaining = request
    ? Math.max(0, DECISION_SECONDS - Math.floor((now - request.receivedAt) / 1000))
    : 0;

  useEffect(() => {
    if (request && remaining === 0) onAnswer(request.requestId, false);
  }, [request, remaining, onAnswer]);

  return (
    <Dialog
      open={Boolean(request)}
      onOpenChange={(open) => {
        // Fechar o diálogo (Esc) conta como recusa.
        if (!open && request) onAnswer(request.requestId, false);
      }}
    >
      {request && (
        <DialogContent onPointerDownOutside={(e) => e.preventDefault()}>
          <DialogHeader>
            <div className="flex items-center gap-2 text-amber-500">
              <ShieldAlert className="size-5" />
              <DialogTitle className="text-foreground">Pedido de acesso</DialogTitle>
            </div>
            <DialogDescription>
              <strong className="text-foreground">{request.fromName}</strong> (ID{" "}
              <span className="font-mono">{formatId(request.fromId)}</span>) quer acessar este
              computador. Só aceite se você conhece e confia nesta pessoa.
            </DialogDescription>
          </DialogHeader>
          <label className="flex items-start gap-2 text-sm">
            <input
              type="checkbox"
              className="mt-0.5 size-4 accent-primary"
              checked={allowControl}
              onChange={(e) => setAllowControl(e.target.checked)}
            />
            <span>
              Permitir controle do mouse e do teclado
              <span className="block text-xs text-muted-foreground">
                Desmarcado, a pessoa só vê a tela. Dá para mudar durante a sessão.
              </span>
            </span>
          </label>
          <DialogFooter>
            <Button variant="outline" onClick={() => onAnswer(request.requestId, false)}>
              Recusar ({remaining}s)
            </Button>
            <Button onClick={() => onAnswer(request.requestId, true, allowControl)}>Aceitar</Button>
          </DialogFooter>
        </DialogContent>
      )}
    </Dialog>
  );
}
