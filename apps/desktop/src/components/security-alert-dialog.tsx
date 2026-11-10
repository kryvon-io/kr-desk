import { ShieldX } from "lucide-react";

import { Button } from "@/components/ui/button";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { formatId, type SecurityAlert } from "@/lib/signaling-state";

/** Sessão recusada por falha na verificação de identidade. */
export function SecurityAlertDialog({
  alert,
  onClose,
  onTrustNew,
}: {
  alert: SecurityAlert | null;
  onClose: () => void;
  onTrustNew: (peerId: string) => void;
}) {
  return (
    <Dialog open={alert !== null} onOpenChange={(open) => !open && onClose()}>
      {alert && (
        <DialogContent>
          <DialogHeader>
            <div className="flex items-center gap-2 text-destructive">
              <ShieldX className="size-5" />
              <DialogTitle className="text-foreground">Conexão bloqueada por segurança</DialogTitle>
            </div>
            <DialogDescription>
              {alert.reason}
              {alert.keyChanged ? (
                <>
                  {" "}
                  Isso acontece quando o KR Desk é reinstalado em{" "}
                  <span className="font-mono">{formatId(alert.peerId)}</span>, mas também pode
                  indicar alguém tentando se passar por ele. Só confie se tiver certeza — por
                  exemplo, confirmando com a pessoa por telefone.
                </>
              ) : (
                <> Nenhuma imagem, tecla ou clique foi trocado com o outro computador.</>
              )}
            </DialogDescription>
          </DialogHeader>
          <DialogFooter>
            {alert.keyChanged && (
              <Button variant="outline" onClick={() => onTrustNew(alert.peerId)}>
                Confiar na nova identidade
              </Button>
            )}
            <Button onClick={onClose}>Fechar</Button>
          </DialogFooter>
        </DialogContent>
      )}
    </Dialog>
  );
}
