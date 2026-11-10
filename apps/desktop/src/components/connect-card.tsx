import { CircleAlert, CircleCheck, Loader2, PlugZap, X } from "lucide-react";
import { useState, type FormEvent } from "react";

import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { formatId, sanitizeIdInput, type Outgoing } from "@/lib/signaling-state";

interface Props {
  outgoing: Outgoing;
  online: boolean;
  onConnect: (targetId: string) => void;
  onCancel: (requestId: string) => void;
  onReset: () => void;
}

/** Formulário para pedir acesso a outro computador e acompanhar o pedido. */
export function ConnectCard({ outgoing, online, onConnect, onCancel, onReset }: Props) {
  const [value, setValue] = useState("");
  const digits = sanitizeIdInput(value);
  const busy = outgoing.kind === "sending" || outgoing.kind === "pending";

  function submit(e: FormEvent) {
    e.preventDefault();
    if (digits.length === 9 && online && !busy) onConnect(digits);
  }

  return (
    <section className="rounded-xl border bg-card p-6 shadow-sm">
      <h2 className="text-sm text-muted-foreground">Acessar outro computador</h2>
      <form className="mt-3 flex gap-2" onSubmit={submit}>
        <Input
          inputMode="numeric"
          autoComplete="off"
          placeholder="ID do outro computador"
          aria-label="ID do outro computador"
          className="font-mono text-lg tracking-wider placeholder:font-sans placeholder:text-sm placeholder:tracking-normal"
          value={formatId(digits)}
          onChange={(e) => {
            setValue(e.target.value);
            if (outgoing.kind === "failed" || outgoing.kind === "accepted") onReset();
          }}
          disabled={busy}
        />
        <Button type="submit" disabled={digits.length !== 9 || !online || busy}>
          {busy ? <Loader2 className="animate-spin" /> : <PlugZap />}
          Conectar
        </Button>
      </form>

      <div className="mt-3 min-h-6 text-sm" aria-live="polite">
        {outgoing.kind === "pending" && (
          <div className="flex items-center justify-between gap-2 text-muted-foreground">
            <span className="flex items-center gap-2">
              <Loader2 className="size-4 animate-spin" />
              Aguardando autorização em {formatId(outgoing.targetId)}…
            </span>
            <Button variant="ghost" size="sm" onClick={() => onCancel(outgoing.requestId)}>
              <X /> Cancelar
            </Button>
          </div>
        )}
        {outgoing.kind === "accepted" && (
          <p className="flex items-center gap-2 text-emerald-600 dark:text-emerald-400">
            <CircleCheck className="size-4" />
            Acesso autorizado por {formatId(outgoing.peerId)}. A transmissão de tela chega na
            próxima etapa.
          </p>
        )}
        {outgoing.kind === "failed" && (
          <p className="flex items-center gap-2 text-destructive">
            <CircleAlert className="size-4" />
            {outgoing.message}
          </p>
        )}
      </div>
    </section>
  );
}
