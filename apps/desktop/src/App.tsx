import { MonitorSmartphone, PlugZap, ShieldCheck } from "lucide-react";
import { useEffect, useState } from "react";

import { Button } from "@/components/ui/button";
import { fetchAppInfo, type AppInfo } from "@/lib/app-info";

export default function App() {
  const [info, setInfo] = useState<AppInfo | null>(null);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    fetchAppInfo()
      .then(setInfo)
      .catch((e: unknown) => setError(String(e)));
  }, []);

  return (
    <main className="flex min-h-screen flex-col items-center justify-center gap-8 p-8">
      <div className="flex items-center gap-3">
        <MonitorSmartphone className="size-10" aria-hidden />
        <h1 className="text-3xl font-semibold tracking-tight">KR Desk</h1>
      </div>

      <section className="w-full max-w-md rounded-xl border bg-card p-6 shadow-sm">
        <p className="text-sm text-muted-foreground">Seu ID</p>
        <p className="mt-1 font-mono text-2xl tracking-widest text-muted-foreground">
          — — — — — — — — —
        </p>
        <p className="mt-2 text-xs text-muted-foreground">
          O ID será atribuído quando o servidor de sinalização estiver disponível (Etapa 1).
        </p>
        <Button className="mt-6 w-full" disabled>
          <PlugZap /> Conectar a outro computador
        </Button>
      </section>

      <footer className="flex items-center gap-2 text-xs text-muted-foreground">
        <ShieldCheck className="size-3.5" aria-hidden />
        {info ? (
          <span>
            v{info.version} · protocolo v{info.protocolVersion} · {info.targetOs}/{info.targetArch}
          </span>
        ) : error ? (
          <span>Erro ao consultar o núcleo: {error}</span>
        ) : (
          <span>Modo navegador (fora do Tauri)</span>
        )}
      </footer>
    </main>
  );
}
