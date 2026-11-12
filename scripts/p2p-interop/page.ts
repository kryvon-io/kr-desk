// Página de teste: o mesmo `ViewerPeer` do app, rodando num Chrome comum.
// Sucesso = identidade do host verificada (Ed25519 via WebCrypto), latência
// medida, rota informada, quadros H.264 decodificados, cursor recebido, troca
// de monitor e entrada (mouse/teclado) entregue ao host.
// Com `?tamper=1`, o host assina um fingerprint falso e o esperado é a recusa.
import {
  base64UrlDecode,
  base64UrlEncode,
  type SessionCrypto,
} from "../../apps/desktop/src/lib/session-auth";
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
  auth?: string;
  clipboard?: string;
  rxReports?: number;
  reconnected?: boolean;
  code?: string;
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
/** O mesmo identificador de sessão usado pelo host de teste. */
const REQUEST_ID = "interop-session";

/** Assinatura com uma chave Ed25519 efêmera e verificação no próprio Chrome. */
async function webCryptoSession(): Promise<SessionCrypto> {
  const keys = (await crypto.subtle.generateKey({ name: "Ed25519" }, true, [
    "sign",
    "verify",
  ])) as CryptoKeyPair;
  const publicKey = base64UrlEncode(
    new Uint8Array(await crypto.subtle.exportKey("raw", keys.publicKey)),
  );
  const encode = (m: string) => new TextEncoder().encode(m);
  return {
    sign: async (message) => ({
      publicKey,
      signature: base64UrlEncode(
        new Uint8Array(
          await crypto.subtle.sign({ name: "Ed25519" }, keys.privateKey, encode(message)),
        ),
      ),
    }),
    verify: async (pk, sig, message) => {
      const key = await crypto.subtle.importKey(
        "raw",
        base64UrlDecode(pk),
        { name: "Ed25519" },
        false,
        ["verify"],
      );
      const ok = await crypto.subtle.verify(
        { name: "Ed25519" },
        key,
        base64UrlDecode(sig),
        encode(message),
      );
      return ok
        ? { result: "ok", firstContact: true }
        : { result: "failed", reason: "assinatura inválida" };
    },
  };
}

ws.onopen = async () => {
  const sessionCrypto = await webCryptoSession();
  const video = document.createElement("video");
  video.muted = true;
  video.autoplay = true;
  video.playsInline = true;
  document.body.append(video);

  const peer = new ViewerPeer(
    [],
    {
      onSignal: (p) => ws.send(JSON.stringify(p)),
      onState: (s) => (result.state = s),
      onRtt: (ms) => (result.rttMs = ms),
      onRoute: (r) => (result.route = `${r.local}/${r.remote}/${r.protocol}`),
      onCursor: (msg) => {
        if (msg.t === "cursor_shape") result.cursor = `${msg.w}x${msg.h}`;
      },
      onMonitors: (list) => (result.monitors = list.length),
      onVerificationCode: (code) => (result.code = code),
      onClipboard: (text) => (result.clipboard = text),
      onRxReport: () => (result.rxReports = (result.rxReports ?? 0) + 1),
      onAuth: (outcome) => {
        if (outcome.result === "ok") {
          result.auth = "ok";
        } else {
          result.error = `auth: ${outcome.result === "failed" ? outcome.reason : "chave trocada"}`;
          ws.close();
        }
      },
      onVideo: (stream) => {
        video.srcObject = stream;
        void video.play().catch(() => undefined);
      },
    },
    { auth: { requestId: REQUEST_ID, crypto: sessionCrypto, password: "senha-interop" } },
  );
  ws.onmessage = (e) => {
    peer.handleSignal(JSON.parse(String(e.data)) as SignalPayload).catch((err: unknown) => {
      result.error = String(err);
    });
  };
  peer.start().catch((err: unknown) => (result.error = String(err)));

  let switchRequested: number | null = null;
  setInterval(() => {
    void sampleVideo(peer.pc).then((s) => {
      if (!s) return;
      result.framesDecoded = s.framesDecoded;
      if (s.frameWidth) result.resolution = `${s.frameWidth}x${s.frameHeight}`;
      result.decoder = s.decoderImplementation;
      const ready =
        result.auth === "ok" &&
        result.rttMs !== undefined &&
        result.route !== undefined &&
        s.framesDecoded >= MIN_FRAMES &&
        result.cursor !== undefined &&
        result.clipboard === "olá do host ✓" &&
        (result.rxReports ?? 0) >= 2 &&
        result.monitors === 2;
      if (ready && switchRequested === null) {
        // Troca para o segundo monitor (320×240) e espera a nova resolução.
        switchRequested = s.framesDecoded;
        peer.selectMonitor(1);
      } else if (switchRequested !== null && result.resolution === "320x240" && !result.ok) {
        result.switchedTo = result.resolution;
        result.ok = true;
        // As estatísticas continuam (o teste de reconexão usa framesDecoded).
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
  peer.sendClipboard("olá do viewer ç");
  // Reinício de ICE: a conexão tem de voltar e o vídeo continuar chegando.
  const before = result.framesDecoded ?? 0;
  peer.reconnectNow();
  const wait = setInterval(() => {
    if (result.state === "connected" && (result.framesDecoded ?? 0) >= before + 10) {
      clearInterval(wait);
      result.reconnected = true;
      ws.send("done");
    }
  }, 250);
}
