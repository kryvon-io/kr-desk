import {
  Activity,
  Monitor,
  Loader2,
  Maximize,
  Minimize,
  Power,
  Router,
  TriangleAlert,
  Zap,
} from "lucide-react";
import { useCallback, useEffect, useRef, useState } from "react";

import { Button } from "@/components/ui/button";
import { formatId, type ActiveSession } from "@/lib/signaling-state";
import { cn } from "@/lib/utils";
import {
  metricsBetween,
  sampleVideo,
  type VideoMetrics,
  type VideoSample,
} from "@/lib/video-stats";
import type { MonitorInfo } from "@/lib/control";
import type { ViewerPeer } from "@/lib/viewer-peer";
import { useRemoteInput } from "@/lib/use-remote-input";

/** Some a barra depois deste tempo (com vídeo na tela). */
const HIDE_BAR_MS = 2500;
/** A barra aparece quando o mouse chega a esta distância do topo. */
const BAR_HOT_ZONE_PX = 48;

function useVideoMetrics(peer: ViewerPeer | null): VideoMetrics | null {
  const [metrics, setMetrics] = useState<VideoMetrics | null>(null);
  useEffect(() => {
    if (!peer) return;
    let prev: VideoSample | null = null;
    let disposed = false;
    const timer = setInterval(() => {
      void sampleVideo(peer.pc).then((cur) => {
        if (disposed || !cur) return;
        setMetrics(metricsBetween(prev, cur));
        prev = cur;
      });
    }, 1000);
    return () => {
      disposed = true;
      clearInterval(timer);
    };
  }, [peer]);
  return metrics;
}

function formatRate(kbps: number): string {
  return kbps >= 1000 ? `${(kbps / 1000).toFixed(1)} Mbps` : `${kbps} kbps`;
}

/** Tela do computador remoto, com barra de métricas e controles. */
export function RemoteScreen({
  session,
  peer,
  stream,
  cursor,
  monitors,
  tauri,
  onEnd,
}: {
  session: ActiveSession;
  peer: ViewerPeer | null;
  stream: MediaStream | null;
  /** Cursor do host (valor CSS). */
  cursor: string;
  /** Monitores do host e o transmitido. */
  monitors: { list: MonitorInfo[]; active: number } | null;
  /** Rodando dentro do app (hook de teclado disponível). */
  tauri: boolean;
  onEnd: (requestId: string) => void;
}) {
  const container = useRef<HTMLDivElement>(null);
  const video = useRef<HTMLVideoElement>(null);
  const [playing, setPlaying] = useState(false);
  const [barVisible, setBarVisible] = useState(true);
  const [fullscreen, setFullscreen] = useState(false);
  const [details, setDetails] = useState(false);
  const hideTimer = useRef<ReturnType<typeof setTimeout> | undefined>(undefined);
  const metrics = useVideoMetrics(peer);
  useRemoteInput(video, peer, playing, tauri);

  useEffect(() => {
    const el = video.current;
    if (!el) return;
    el.srcObject = stream;
    if (stream) {
      void el.play().catch(() => undefined);
      // O primeiro quadro precisa ser um keyframe; pede um ao montar.
      peer?.requestKeyframe();
    }
  }, [stream, peer]);

  useEffect(() => {
    const onChange = () => setFullscreen(document.fullscreenElement !== null);
    document.addEventListener("fullscreenchange", onChange);
    return () => document.removeEventListener("fullscreenchange", onChange);
  }, []);

  const showBar = useCallback(() => {
    setBarVisible(true);
    clearTimeout(hideTimer.current);
    hideTimer.current = setTimeout(() => setBarVisible(false), HIDE_BAR_MS);
  }, []);
  useEffect(() => () => clearTimeout(hideTimer.current), []);

  const toggleFullscreen = () => {
    if (document.fullscreenElement) void document.exitFullscreen();
    else void container.current?.requestFullscreen();
  };

  const p2p = session.p2p;
  const failed = p2p.state === "failed";
  const relayed = p2p.route?.relayed ?? false;
  const barShown = barVisible || !playing || failed;

  return (
    <div
      ref={container}
      className="relative flex h-screen w-screen items-center justify-center overflow-hidden bg-black text-white"
      onMouseMove={(e) => {
        if (e.clientY <= BAR_HOT_ZONE_PX) showBar();
      }}
    >
      <video
        ref={video}
        tabIndex={-1}
        className={cn("size-full object-contain outline-none", !playing && "invisible")}
        style={{ cursor: playing ? cursor : "default" }}
        muted
        playsInline
        autoPlay
        onPlaying={() => {
          setPlaying(true);
          showBar();
        }}
      />

      {!playing && (
        <div className="absolute flex flex-col items-center gap-3 text-sm text-white/80">
          {failed ? (
            <>
              <TriangleAlert className="size-8 text-red-400" />
              Não foi possível conectar os dois computadores.
            </>
          ) : (
            <>
              <Loader2 className="size-8 animate-spin" />
              {p2p.state === "connected"
                ? "Aguardando a imagem da tela remota…"
                : "Estabelecendo conexão direta…"}
            </>
          )}
        </div>
      )}

      <div
        className={cn(
          // O fundo não bloqueia cliques na tela remota; só o conteúdo da barra.
          "pointer-events-none absolute inset-x-0 top-0 flex items-center justify-between gap-3 bg-gradient-to-b from-black/80 to-transparent px-4 pt-3 pb-6 text-xs transition-opacity duration-300",
          barShown ? "opacity-100 [&>*]:pointer-events-auto" : "opacity-0",
        )}
      >
        <div className="flex min-w-0 flex-wrap items-center gap-x-4 gap-y-1">
          <span className="font-mono text-sm whitespace-nowrap">{formatId(session.peerId)}</span>
          <span className="flex items-center gap-1.5 whitespace-nowrap text-white/80">
            {relayed ? (
              <Router className="size-3.5" />
            ) : (
              <Zap className="size-3.5 text-emerald-400" />
            )}
            {relayed ? "Via servidor" : "Direta"}
            {p2p.rttMs !== null && ` · ${p2p.rttMs} ms`}
          </span>
          {metrics && playing && (
            <span className="flex items-center gap-1.5 whitespace-nowrap text-white/80">
              <Activity className="size-3.5" />
              {metrics.fps} fps · {formatRate(metrics.kbps)}
              {metrics.width && ` · ${metrics.width}×${metrics.height}`}
            </span>
          )}
          {details && metrics && (
            <span className="whitespace-nowrap text-white/60">
              decodificação {metrics.decodeMs ?? "–"} ms · buffer {metrics.jitterMs ?? "–"} ms ·
              perdidos {metrics.packetsLost} · PLI {metrics.pliCount}
              {metrics.decoder && ` · ${metrics.decoder}`}
            </span>
          )}
        </div>
        <div className="flex shrink-0 items-center gap-1">
          {monitors && monitors.list.length > 1 && (
            <div
              className="mr-2 flex items-center gap-1"
              role="group"
              aria-label="Telas do computador remoto"
            >
              {monitors.list.map((m, i) => {
                const active = m.index === monitors.active;
                return (
                  <Button
                    key={m.index}
                    variant="ghost"
                    size="sm"
                    className={cn(
                      "gap-1.5 text-white hover:bg-white/15 hover:text-white",
                      active && "bg-white/20",
                    )}
                    aria-pressed={active}
                    title={`${m.primary ? "Tela principal" : `Tela ${i + 1}`} · ${m.width}×${m.height}`}
                    onClick={() => {
                      if (!active) peer?.selectMonitor(m.index);
                    }}
                  >
                    <Monitor className="size-3.5" />
                    {i + 1}
                  </Button>
                );
              })}
            </div>
          )}
          <Button
            variant="ghost"
            size="icon"
            className="text-white hover:bg-white/15 hover:text-white"
            onClick={() => setDetails((d) => !d)}
            aria-label="Detalhes da conexão"
            aria-pressed={details}
          >
            <Activity />
          </Button>
          <Button
            variant="ghost"
            size="icon"
            className="text-white hover:bg-white/15 hover:text-white"
            onClick={toggleFullscreen}
            aria-label={fullscreen ? "Sair da tela cheia" : "Tela cheia"}
          >
            {fullscreen ? <Minimize /> : <Maximize />}
          </Button>
          <Button
            size="sm"
            className="bg-red-600 text-white hover:bg-red-500"
            onClick={() => onEnd(session.requestId)}
          >
            <Power /> Encerrar
          </Button>
        </div>
      </div>
    </div>
  );
}
