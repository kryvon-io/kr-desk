import { invoke } from "@tauri-apps/api/core";
import { CircleAlert, KeyRound, Loader2, ShieldCheck, ShieldOff } from "lucide-react";
import { useCallback, useEffect, useState, type FormEvent } from "react";

import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";

interface UnattendedStatus {
  enabled: boolean;
  /** Com o serviço, só administradores deste computador podem mudar. */
  canConfigure: boolean;
  /** Vale para o computador (serviço) ou só para este app. */
  machineWide: boolean;
}

const MIN_CHARS = 8;

/** Liga/desliga o acesso não assistido (entrar com senha, sem ninguém aceitar). */
export function UnattendedCard() {
  const [status, setStatus] = useState<UnattendedStatus | null>(null);
  const [editing, setEditing] = useState(false);
  const [password, setPassword] = useState("");
  const [confirm, setConfirm] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const refresh = useCallback(() => {
    invoke<UnattendedStatus>("unattended_status")
      .then((s) => {
        setStatus(s);
        setError(null);
      })
      .catch((err: unknown) => setError(String(err)));
  }, []);

  useEffect(refresh, [refresh]);

  const tooShort = password.length > 0 && [...password].length < MIN_CHARS;
  const mismatch = confirm.length > 0 && confirm !== password;
  const valid = [...password].length >= MIN_CHARS && confirm === password;

  async function save(e: FormEvent) {
    e.preventDefault();
    if (!valid || busy) return;
    setBusy(true);
    setError(null);
    try {
      await invoke("set_unattended_password", { password });
      setEditing(false);
      setPassword("");
      setConfirm("");
      refresh();
    } catch (err) {
      setError(String(err));
    } finally {
      setBusy(false);
    }
  }

  async function disable() {
    setBusy(true);
    setError(null);
    try {
      await invoke("disable_unattended");
      refresh();
    } catch (err) {
      setError(String(err));
    } finally {
      setBusy(false);
    }
  }

  if (!status) {
    return error ? (
      <section className="rounded-xl border bg-card p-6 text-sm text-muted-foreground shadow-sm">
        Acesso não assistido indisponível: {error}
      </section>
    ) : null;
  }

  return (
    <section className="rounded-xl border bg-card p-6 shadow-sm">
      <div className="flex items-center justify-between gap-2">
        <h2 className="text-sm text-muted-foreground">Acesso não assistido</h2>
        <span
          className={
            status.enabled
              ? "flex items-center gap-1 text-xs text-emerald-600 dark:text-emerald-400"
              : "flex items-center gap-1 text-xs text-muted-foreground"
          }
        >
          {status.enabled ? (
            <ShieldCheck className="size-3.5" />
          ) : (
            <ShieldOff className="size-3.5" />
          )}
          {status.enabled ? "Ativado" : "Desativado"}
        </span>
      </div>
      <p className="mt-2 text-sm text-muted-foreground">
        Com uma senha, você entra neste computador sem ninguém precisar aceitar
        {status.machineWide
          ? ", inclusive na tela de login."
          : " (com o KR Desk aberto). Para funcionar também na tela de login, instale “para todos os usuários”."}
      </p>

      {!status.canConfigure && (
        <p className="mt-3 text-xs text-muted-foreground">
          Só administradores deste computador podem mudar esta configuração.
        </p>
      )}

      {status.canConfigure && !editing && (
        <div className="mt-3 flex gap-2">
          <Button size="sm" variant="outline" onClick={() => setEditing(true)} disabled={busy}>
            <KeyRound />
            {status.enabled ? "Trocar senha" : "Definir senha"}
          </Button>
          {status.enabled && (
            <Button size="sm" variant="ghost" onClick={() => void disable()} disabled={busy}>
              Desativar
            </Button>
          )}
        </div>
      )}

      {status.canConfigure && editing && (
        <form className="mt-3 flex flex-col gap-2" onSubmit={(e) => void save(e)}>
          <Input
            type="password"
            autoComplete="new-password"
            placeholder={`Nova senha (mínimo ${MIN_CHARS} caracteres)`}
            aria-label="Nova senha"
            value={password}
            onChange={(e) => setPassword(e.target.value)}
            disabled={busy}
          />
          <Input
            type="password"
            autoComplete="new-password"
            placeholder="Repita a senha"
            aria-label="Repita a senha"
            value={confirm}
            onChange={(e) => setConfirm(e.target.value)}
            disabled={busy}
          />
          {(tooShort || mismatch) && (
            <p className="text-xs text-destructive">
              {tooShort
                ? `A senha precisa ter pelo menos ${MIN_CHARS} caracteres.`
                : "As senhas não são iguais."}
            </p>
          )}
          <div className="flex gap-2">
            <Button size="sm" type="submit" disabled={!valid || busy}>
              {busy && <Loader2 className="animate-spin" />}
              Salvar e ativar
            </Button>
            <Button
              size="sm"
              variant="ghost"
              type="button"
              onClick={() => {
                setEditing(false);
                setPassword("");
                setConfirm("");
              }}
              disabled={busy}
            >
              Cancelar
            </Button>
          </div>
        </form>
      )}

      {error && (
        <p className="mt-3 flex items-center gap-2 text-sm text-destructive">
          <CircleAlert className="size-4" /> {error}
        </p>
      )}
    </section>
  );
}
