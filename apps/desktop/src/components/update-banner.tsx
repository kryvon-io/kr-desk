import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { CircleAlert, Download, Loader2, X } from "lucide-react";
import { useEffect, useState } from "react";

import { Button } from "@/components/ui/button";

interface UpdateInfo {
  version: string;
  notes: string | null;
}

/** Avisa quando há versão nova e instala com um clique (o app reinicia). */
export function UpdateBanner() {
  const [update, setUpdate] = useState<UpdateInfo | null>(null);
  const [dismissed, setDismissed] = useState(false);
  const [installing, setInstalling] = useState(false);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    let unlisten: (() => void) | undefined;
    let disposed = false;
    invoke<UpdateInfo | null>("check_update")
      .then((u) => {
        if (!disposed && u) setUpdate(u);
      })
      .catch(() => undefined);
    void listen<UpdateInfo>("update-available", (e) => setUpdate(e.payload)).then((fn) => {
      if (disposed) fn();
      else unlisten = fn;
    });
    return () => {
      disposed = true;
      unlisten?.();
    };
  }, []);

  if (!update || dismissed) return null;

  async function install() {
    setInstalling(true);
    setError(null);
    try {
      // Se der certo, o app fecha e o instalador abre a versão nova.
      await invoke("install_update");
    } catch (err) {
      setError(String(err));
      setInstalling(false);
    }
  }

  return (
    <div className="flex w-full max-w-md flex-col gap-2 rounded-lg border border-primary/40 bg-primary/5 px-4 py-3 text-sm">
      <div className="flex items-center justify-between gap-2">
        <span className="flex items-center gap-2">
          <Download className="size-4" />
          Nova versão disponível: <strong>v{update.version}</strong>
        </span>
        {!installing && (
          <button
            onClick={() => setDismissed(true)}
            aria-label="Lembrar depois"
            title="Lembrar depois"
            className="opacity-60"
          >
            <X className="size-4" />
          </button>
        )}
      </div>
      <div className="flex items-center gap-2">
        <Button size="sm" onClick={() => void install()} disabled={installing}>
          {installing ? <Loader2 className="animate-spin" /> : <Download />}
          {installing ? "Baixando…" : "Atualizar e reiniciar"}
        </Button>
        <span className="text-xs text-muted-foreground">
          Sessões em andamento serão encerradas.
        </span>
      </div>
      {error && (
        <p className="flex items-center gap-2 text-destructive">
          <CircleAlert className="size-4" /> {error}
        </p>
      )}
    </div>
  );
}
