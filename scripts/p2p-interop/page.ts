// Página de teste: o mesmo `ViewerPeer` do app, rodando num Chrome comum.
// Sucesso = latência medida, rota informada, quadros H.264 decodificados,
// cursor recebido e entrada (mouse/teclado) entregue ao host.
import { sampleVideo } from "../../apps/desktop/src/lib/video-stats";
import { ViewerPeer, type SignalPayload } from "../../apps/desktop/src/lib/viewer-peer";

interface InteropResult {
  ok: boolean;
  state?: string;
  rttMs?: number;
  route?: string;
  framesDecoded?: number;
  resolution?: string;
  decoder?: string;
  cursor?: string;
  monitors?: number;
  switchedTo?: string;
  error?: string;
}

declare global {
  interface Window {
    __interop: InteropResult;
  }
}

/** Quadros decodificados exigidos para considerar o vídeo funcionando. */
const MIN_FRAMES = 15;

const result: InteropResult = { ok: false };
window.__interop = result;
const ws = new WebSocket(new URLSearchParams(location.search).get("ws") ?? "");

ws.onopen = () => {
  const video = document.createElement("video");
  video.muted = true;
  video.autoplay = true;
  video.playsInline = true;
  document.body.append(video);

  const peer = new ViewerPeer([], {
    onSignal: (p) => ws.send(JSON.stringify(p)),
    onState: (s) => (result.state = s),
    onRtt: (ms) => (result.rttMs = ms),
    onRoute: (r) => (result.route = `${r.local}/${r.remote}/${r.protocol}`),
    onCursor: (msg) => {
      if (msg.t === "cursor_shape") result.cursor = `${msg.w}x${msg.h}`;
    },
    onMonitors: (list) => (result.monitors = list.length),
    onVideo: (stream) => {
      video.srcObject = stream;
      void video.play().catch(() => undefined);
    },
  });
  ws.onmessage = (e) => {
    peer.handleSignal(JSON.parse(String(e.data)) as SignalPayload).catch((err: unknown) => {
      result.error = String(err);
    });
  };
  peer.start().catch((err: unknown) => (result.error = String(err)));

  let switchRequested: number | null = null;
  const timer = setInterval(() => {
    void sampleVideo(peer.pc).then((s) => {
      if (!s) return;
      result.framesDecoded = s.framesDecoded;
      if (s.frameWidth) result.resolution = `${s.frameWidth}x${s.frameHeight}`;
      result.decoder = s.decoderImplementation;
      const ready =
        result.rttMs !== undefined &&
        result.route !== undefined &&
        s.framesDecoded >= MIN_FRAMES &&
        result.cursor !== undefined &&
        result.monitors === 2;
      if (ready && switchRequested === null) {
        // Troca para o segundo monitor (320×240) e espera a nova resolução.
        switchRequested = s.framesDecoded;
        peer.selectMonitor(1);
      } else if (switchRequested !== null && result.resolution === "320x240" && !result.ok) {
        result.switchedTo = result.resolution;
        result.ok = true;
        clearInterval(timer);
        sendInputAndFinish(peer);
      }
    });
  }, 250);
};
ws.onerror = () => (result.error = "falha no WebSocket");

function sendInputAndFinish(peer: ViewerPeer) {
  for (let i = 0; i < 5; i++) peer.sendInput({ t: "mouse_move", x: 1000 * i, y: 2000 });
  peer.sendInput({ t: "mouse_button", button: "left", down: true, x: 5000, y: 2000 });
  peer.sendInput({ t: "mouse_button", button: "left", down: false, x: 5000, y: 2000 });
  peer.sendInput({ t: "wheel", dx: 0, dy: -120 });
  peer.sendInput({ t: "key", code: 0x1e, ext: false, down: true });
  peer.sendInput({ t: "key", code: 0x1e, ext: false, down: false });
  // Dá tempo de o canal não confiável entregar antes de encerrar.
  setTimeout(() => ws.send("done"), 500);
}
