/**
 * Lado **viewer** da conexão P2P, com o WebRTC nativo do WebView2 (Chromium).
 *
 * Cria a oferta e o DataChannel `control`; troca SDP/candidatos com o host
 * (webrtc-rs) pela sinalização. Independe de React e de Tauri para poder ser
 * testado num Chrome comum (`scripts/p2p-interop`).
 */
import { CONTROL_LABEL, isCandidateType, parseControl, type CandidateType } from "./control";

/** Formato de `payload` de `signal` (igual a `kr_transport::SignalPayload`). */
export type SignalPayload =
  | { kind: "offer"; sdp: string }
  | { kind: "answer"; sdp: string }
  | { kind: "candidate"; candidate: RTCIceCandidateInit };

export interface IceServerDto {
  urls: string[];
  username?: string;
  credential?: string;
}

export type PeerState = "connecting" | "connected" | "disconnected" | "failed" | "closed";

export interface RouteInfo {
  local: CandidateType;
  remote: CandidateType;
  protocol: string;
  relayed: boolean;
}

export interface ViewerPeerEvents {
  onSignal: (payload: SignalPayload) => void;
  onState?: (state: PeerState) => void;
  onRtt?: (ms: number) => void;
  onRoute?: (route: RouteInfo) => void;
  onControlOpen?: () => void;
}

const PING_INTERVAL_MS = 1000;

export class ViewerPeer {
  readonly pc: RTCPeerConnection;
  private readonly control: RTCDataChannel;
  private readonly timers: ReturnType<typeof setInterval>[] = [];
  private seq = 1;
  private inFlight: { n: number; at: number } | null = null;
  private pendingCandidates: RTCIceCandidateInit[] = [];
  private lastRouteKey = "";
  private lastSentRouteKey = "";
  private closed = false;

  constructor(
    iceServers: IceServerDto[],
    private readonly events: ViewerPeerEvents,
    options: { forceRelay?: boolean } = {},
  ) {
    this.pc = new RTCPeerConnection({
      iceServers,
      ...(options.forceRelay ? { iceTransportPolicy: "relay" as const } : {}),
    });
    this.control = this.pc.createDataChannel(CONTROL_LABEL, { ordered: true });

    this.pc.onicecandidate = (e) => {
      if (e.candidate) this.events.onSignal({ kind: "candidate", candidate: e.candidate.toJSON() });
    };
    this.pc.onconnectionstatechange = () => {
      const s = this.pc.connectionState;
      this.events.onState?.(s === "new" ? "connecting" : (s as PeerState));
      if (s === "connected") void this.reportRoute();
    };

    this.control.onopen = () => {
      this.events.onControlOpen?.();
      void this.reportRoute();
      this.timers.push(setInterval(() => this.ping(), PING_INTERVAL_MS));
      // A rota pode mudar (ex.: de relay para direta); reavalia periodicamente.
      this.timers.push(setInterval(() => void this.reportRoute(), 5000));
    };
    this.control.onmessage = (e) => this.onControl(String(e.data));
    this.control.onclose = () => this.stopTimers();
  }

  /** Inicia a negociação: cria e envia a oferta. */
  async start(): Promise<void> {
    const offer = await this.pc.createOffer();
    await this.pc.setLocalDescription(offer);
    this.events.onSignal({ kind: "offer", sdp: offer.sdp ?? "" });
  }

  /** Processa um sinal vindo do host. */
  async handleSignal(payload: SignalPayload): Promise<void> {
    if (this.closed) return;
    if (payload.kind === "answer") {
      await this.pc.setRemoteDescription({ type: "answer", sdp: payload.sdp });
      for (const c of this.pendingCandidates.splice(0)) await this.pc.addIceCandidate(c);
    } else if (payload.kind === "candidate") {
      if (this.pc.remoteDescription) await this.pc.addIceCandidate(payload.candidate);
      else this.pendingCandidates.push(payload.candidate);
    }
  }

  private ping() {
    if (this.control.readyState !== "open") return;
    const n = this.seq++;
    this.inFlight = { n, at: performance.now() };
    this.control.send(JSON.stringify({ t: "ping", n }));
  }

  private onControl(raw: string) {
    const msg = parseControl(raw);
    if (!msg) return;
    if (msg.t === "ping") this.control.send(JSON.stringify({ t: "pong", n: msg.n }));
    else if (msg.t === "pong" && this.inFlight?.n === msg.n) {
      this.events.onRtt?.(Math.round(performance.now() - this.inFlight.at));
      this.inFlight = null;
    }
  }

  /** Lê o par de candidatos selecionado em `getStats()` e informa o host. */
  private async reportRoute() {
    const route = await selectedRoute(this.pc);
    if (!route) return;
    const key = `${route.local}/${route.remote}/${route.protocol}`;
    if (key !== this.lastRouteKey) {
      this.lastRouteKey = key;
      this.events.onRoute?.(route);
    }
    // Envia ao host só quando o canal está aberto e a rota mudou desde o último envio.
    if (this.control.readyState === "open" && key !== this.lastSentRouteKey) {
      this.lastSentRouteKey = key;
      this.control.send(
        JSON.stringify({
          t: "route",
          local: route.local,
          remote: route.remote,
          protocol: route.protocol,
        }),
      );
    }
  }

  private stopTimers() {
    this.timers.splice(0).forEach(clearInterval);
  }

  close() {
    if (this.closed) return;
    this.closed = true;
    this.stopTimers();
    this.control.close();
    this.pc.close();
  }
}

/** Rota em uso segundo `getStats()` (par de candidatos nomeado e bem-sucedido). */
export async function selectedRoute(pc: RTCPeerConnection): Promise<RouteInfo | null> {
  const stats = await pc.getStats();
  let pairId: string | undefined;
  stats.forEach((s: { type: string; selectedCandidatePairId?: string }) => {
    if (s.type === "transport" && s.selectedCandidatePairId) pairId = s.selectedCandidatePairId;
  });
  let pair: { localCandidateId: string; remoteCandidateId: string } | undefined;
  stats.forEach(
    (s: {
      id: string;
      type: string;
      nominated?: boolean;
      state?: string;
      localCandidateId: string;
      remoteCandidateId: string;
    }) => {
      if (s.type !== "candidate-pair") return;
      if (pairId ? s.id === pairId : s.nominated && s.state === "succeeded") pair = s;
    },
  );
  if (!pair) return null;
  const local = stats.get(pair.localCandidateId) as
    { candidateType?: string; protocol?: string } | undefined;
  const remote = stats.get(pair.remoteCandidateId) as { candidateType?: string } | undefined;
  if (!isCandidateType(local?.candidateType) || !isCandidateType(remote?.candidateType))
    return null;
  return {
    local: local.candidateType,
    remote: remote.candidateType,
    protocol: local.protocol ?? "udp",
    relayed: local.candidateType === "relay" || remote.candidateType === "relay",
  };
}
