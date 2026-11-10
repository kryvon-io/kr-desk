import { Info, MonitorSmartphone, ShieldCheck, X } from "lucide-react";
import { useEffect, useState } from "react";

import { ConnectCard } from "@/components/connect-card";
import { IncomingRequestDialog } from "@/components/incoming-request-dialog";
import { MyIdCard } from "@/components/my-id-card";
import { SessionBanner } from "@/components/session-banner";
import { StatusBadge } from "@/components/status-badge";
import { fetchAppInfo, type AppInfo } from "@/lib/app-info";
import { useSignaling } from "@/lib/use-signaling";

export default function App() {
  const [info, setInfo] = useState<AppInfo | null>(null);
  const { state, deviceName, tauri, connectTo, respond, endSession, resetOutgoing, dismissNotice } =
    useSignaling();

  useEffect(() => {
    fetchAppInfo()
      .then(setInfo)
      .catch(() => setInfo(null));
  }, []);

  useEffect(() => {
    if (!state.notice) return;
    const t = setTimeout(dismissNotice, 6000);
    return () => clearTimeout(t);
  }, [state.notice, dismissNotice]);

  return (
    <main className="flex min-h-screen flex-col items-center gap-6 p-8">
      <header className="flex w-full max-w-md items-center justify-between">
        <div className="flex items-center gap-2">
          <MonitorSmartphone className="size-7" aria-hidden />
          <h1 className="text-2xl font-semibold tracking-tight">KR Desk</h1>
        </div>
        <StatusBadge status={state.status} />
      </header>

      {state.active && <SessionBanner session={state.active} onEnd={endSession} />}

      {state.notice && (
        <div className="flex w-full max-w-md items-center justify-between gap-2 rounded-lg border bg-muted px-4 py-2 text-sm">
          <span className="flex items-center gap-2">
            <Info className="size-4" /> {state.notice}
          </span>
          <button onClick={dismissNotice} aria-label="Fechar aviso" className="opacity-60">
            <X className="size-4" />
          </button>
        </div>
      )}

      <div className="flex w-full max-w-md flex-col gap-4">
        <MyIdCard status={state.status} deviceId={state.deviceId} deviceName={deviceName} />
        <ConnectCard
          outgoing={state.outgoing}
          online={state.status.state === "online"}
          inSession={state.active !== null}
          onConnect={connectTo}
          onCancel={endSession}
          onReset={resetOutgoing}
        />
      </div>

      <IncomingRequestDialog request={state.incoming[0]} onAnswer={respond} />

      <footer className="mt-auto flex items-center gap-2 text-xs text-muted-foreground">
        <ShieldCheck className="size-3.5" aria-hidden />
        {tauri && info ? (
          <span>
            v{info.version} · protocolo v{info.protocolVersion}
          </span>
        ) : (
          <span>Modo navegador (fora do app)</span>
        )}
      </footer>
    </main>
  );
}
