import { CircleAlert, CircleCheck, KeyRound, Loader2, PlugZap, X } from "lucide-react";
import { useState, type FormEvent } from "react";

import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { formatId, sanitizeIdInput, type Outgoing } from "@/lib/signaling-state";

interface Props {
  outgoing: Outgoing;
  online: boolean;
  /** Já existe uma sessão ativa: só uma por vez nesta etapa. */
  inSession: boolean;
  /** `password`: senha de acesso não assistido (entra sem ninguém aceitar). */
  onConnect: (targetId: string, password?: string) => void;
  onCancel: (requestId: string) => void;
  onReset: () => void;
}

/** Formulário para pedir acesso a outro computador e acompanhar o pedido. */
export function ConnectCard({ outgoing, online, inSession, onConnect, onCancel, onReset }: Props) {
  const [value, setValue] = useState("");
  const [password, setPassword] = useState("");
  const [usePassword, setUsePassword] = useState(false);
  const digits = sanitizeIdInput(value);
  const busy = outgoing.kind === "sending" || outgoing.kind === "pending";

  function submit(e: FormEvent) {
    e.preventDefault();
    if (digits.length === 9 && online && !busy && !inSession) {
      onConnect(digits, usePassword && password ? password : undefined);
    }
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
        <Button
          type="submit"
          disabled={digits.length !== 9 || !online || busy || inSession}
          title={inSession ? "Encerre a sessão atual para iniciar outra" : undefined}
        >
          {busy ? <Loader2 className="animate-spin" /> : <PlugZap />}
          Conectar
        </Button>
      </form>

      {usePassword ? (
        <div className="mt-2 flex gap-2">
          <Input
            type="password"
            autoComplete="off"
            placeholder="Senha de acesso não assistido"
            aria-label="Senha de acesso não assistido"
            value={password}
            onChange={(e) => setPassword(e.target.value)}
            disabled={busy}
          />
          <Button
            variant="ghost"
            size="icon"
            aria-label="Conectar sem senha"
            title="Conectar sem senha (o outro lado precisa aceitar)"
            onClick={() => {
              setUsePassword(false);
              setPassword("");
            }}
          >
            <X />
          </Button>
        </div>
      ) : (
        <button
          type="button"
          className="mt-2 flex items-center gap-1.5 text-xs text-muted-foreground hover:text-foreground"
          onClick={() => setUsePassword(true)}
        >
          <KeyRound className="size-3.5" />
          Tenho a senha de acesso não assistido
        </button>
      )}

      <div className="mt-3 min-h-6 text-sm" aria-live="polite">
        {outgoing.kind === "pending" && (
          <div className="flex items-center justify-between gap-2 text-muted-foreground">
            <span className="flex items-center gap-2">
              <Loader2 className="size-4 animate-spin" />
              Aguardando autorização em{" "}
              <span className="whitespace-nowrap">{formatId(outgoing.targetId)}</span>…
            </span>
            <Button variant="ghost" size="sm" onClick={() => onCancel(outgoing.requestId)}>
              <X /> Cancelar
            </Button>
          </div>
        )}
        {outgoing.kind === "accepted" && (
          <p className="flex items-center gap-2 text-emerald-600 dark:text-emerald-400">
            <CircleCheck className="size-4" />
            Acesso autorizado por{" "}
            <span className="whitespace-nowrap">{formatId(outgoing.peerId)}</span>. Abrindo a tela
            remota…
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
