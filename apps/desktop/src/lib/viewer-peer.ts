/**
 * Lado **viewer** da conexão P2P, com o WebRTC nativo do WebView2 (Chromium).
 *
 * Cria a oferta, o DataChannel `control` e um transceiver de vídeo só de
 * recebimento (H.264); troca SDP/candidatos com o host (webrtc-rs) pela
 * sinalização. Independe de React e de Tauri para poder ser
 * testado num Chrome comum (`scripts/p2p-interop`).
 */
import {
  CONTROL_LABEL,
  INPUT_LABEL,
  isCandidateType,
  parseControl,
  type CandidateType,
  type ControlMessage,
  type InputMessage,
  type MonitorInfo,
  MAX_CLIPBOARD_CHARS,
} from "./control";
import {
  sdpFingerprint,
  sessionMessage,
  verificationCode,
  type SessionCrypto,
  type VerifyOutcome,
} from "./session-auth";

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

export type PeerState =
  "connecting" | "connected" | "disconnected" | "reconnecting" | "failed" | "closed";

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
  /** Vídeo da tela do host chegou. */
  onVideo?: (stream: MediaStream) => void;
  /** O host mudou a forma ou a visibilidade do cursor. */
  onCursor?: (msg: Extract<ControlMessage, { t: "cursor_shape" | "cursor_visible" }>) => void;
  /** Monitores do host e qual está sendo transmitido. */
  onMonitors?: (list: MonitorInfo[], active: number) => void;
  /** Código de verificação da sessão (6 dígitos). */
  onVerificationCode?: (code: string) => void;
  /** Resultado da verificação da identidade do host. */
  onAuth?: (outcome: VerifyOutcome) => void;
  /** O host permitiu (ou não) o controle de mouse e teclado. */
  onPermissions?: (control: boolean) => void;
  /** Texto copiado no host. */
  onClipboard?: (text: string) => void;
  /** Relatório de recepção enviado ao host (pacotes perdidos/recebidos). */
  onRxReport?: (lost: number, received: number) => void;
}

/** Autenticação da sessão (obrigatória no app). */
export interface ViewerAuth {
  requestId: string;
  crypto: SessionCrypto;
}

const PING_INTERVAL_MS = 1000;
/** Espera após "disconnected" antes de reiniciar o ICE (quedas curtas voltam sozinhas). */
const RESTART_AFTER_MS = 2000;
/** Intervalo entre tentativas de reinício de ICE. */
const RESTART_RETRY_MS = 5000;
/**
 * Desiste da reconexão depois disso (igual ao prazo do host). Quedas de rede
 * reais costumam passar de 1 min até o DNS e a sinalização voltarem.
 */
const RECONNECT_GIVE_UP_MS = 180_000;

/** Intervalo dos relatórios de recepção (qualidade adaptativa). */
const RX_REPORT_MS = 1000;
/** Prazo para o host provar a identidade depois que o canal abre. */
const AUTH_TIMEOUT_MS = 10_000;

export class ViewerPeer {
  readonly pc: RTCPeerConnection;
  private readonly control: RTCDataChannel;
  private readonly input: RTCDataChannel;
  readonly video: RTCRtpTransceiver;
  private readonly auth: ViewerAuth | null;
  private readonly timers: ReturnType<typeof setInterval>[] = [];
  private seq = 1;
  private inFlight: { n: number; at: number } | null = null;
  private pendingCandidates: RTCIceCandidateInit[] = [];
  private lastRouteKey = "";
  private lastSentRouteKey = "";
  private closed = false;
  /** Host verificado: só então mouse/teclado podem ser enviados. */
  private authenticated = false;
  private authDone = false;
  private controlAllowed = false;
  private lastRttMs = 0;
  private lastRx: { lost: number; received: number } | null = null;
  /** Reconexão em andamento: quando começou e o próximo passo agendado. */
  private reconnect: { since: number; timer: ReturnType<typeof setTimeout> } | null = null;
  /** A reconexão já desistiu: não recomeça sozinha (evita ciclo infinito). */
  private gaveUp = false;

  constructor(
    iceServers: IceServerDto[],
    private readonly events: ViewerPeerEvents,
    options: { forceRelay?: boolean; auth?: ViewerAuth } = {},
  ) {
    this.auth = options.auth ?? null;
    this.pc = new RTCPeerConnection({
      iceServers,
      ...(options.forceRelay ? { iceTransportPolicy: "relay" as const } : {}),
    });
    this.control = this.pc.createDataChannel(CONTROL_LABEL, { ordered: true });
    // Movimentos do mouse: sem retransmissão nem ordem (o próximo substitui).
    this.input = this.pc.createDataChannel(INPUT_LABEL, { ordered: false, maxRetransmits: 0 });
    this.video = this.pc.addTransceiver("video", { direction: "recvonly" });
    preferH264(this.video);
    // Latência acima de suavidade: o mínimo de buffer que o decodificador aceitar.
    (this.video.receiver as RTCRtpReceiver & { jitterBufferTarget?: number }).jitterBufferTarget =
      0;
    this.pc.ontrack = (e) => {
      if (e.track.kind === "video")
        this.events.onVideo?.(e.streams[0] ?? new MediaStream([e.track]));
    };

    this.pc.onicecandidate = (e) => {
      if (e.candidate) this.events.onSignal({ kind: "candidate", candidate: e.candidate.toJSON() });
    };
    this.pc.onconnectionstatechange = () => this.onConnectionState();
    this.pc.oniceconnectionstatechange = () => {
      const ice = this.pc.iceConnectionState;
      if (this.reconnect && (ice === "connected" || ice === "completed")) this.finishReconnect();
    };

    this.control.onopen = () => {
      this.events.onControlOpen?.();
      void this.startAuth();
      void this.reportRoute();
      this.timers.push(setInterval(() => this.ping(), PING_INTERVAL_MS));
      // A rota pode mudar (ex.: de relay para direta); reavalia periodicamente.
      this.timers.push(setInterval(() => void this.reportRoute(), 5000));
    };
    this.control.onmessage = (e) => this.onControl(String(e.data));
    this.control.onclose = () => this.stopTimers();
  }

  /** Inicia a negociação: cria e envia a oferta. */
  private onConnectionState() {
    if (this.closed) return;
    const s = this.pc.connectionState;
    if (s === "connected") {
      this.finishReconnect();
      this.events.onState?.("connected");
      void this.reportRoute();
      return;
    }
    if (s === "disconnected" || s === "failed") {
      // Já conectou uma vez: tenta reconectar (troca de rede, queda curta).
      if (this.authDone && this.authenticated) {
        if (this.gaveUp) return;
        if (!this.reconnect) {
          this.events.onState?.("reconnecting");
          this.reconnect = {
            since: Date.now(),
            timer: setTimeout(() => void this.restartIce(), s === "failed" ? 0 : RESTART_AFTER_MS),
          };
        }
        return;
      }
    }
    this.events.onState?.(s === "new" ? "connecting" : (s as PeerState));
  }

  /** A conexão voltou (o estado pode nem ter mudado, se o caminho antigo ainda servia). */
  private finishReconnect() {
    if (!this.reconnect) return;
    clearTimeout(this.reconnect.timer);
    this.reconnect = null;
    // Imagem nova logo de cara, sem esperar o próximo keyframe periódico.
    this.requestKeyframe();
    this.events.onState?.("connected");
  }

  /** Força uma reconexão (reinício de ICE), ex.: depois de trocar de rede. */
  reconnectNow() {
    if (this.closed || this.reconnect || this.gaveUp) return;
    this.events.onState?.("reconnecting");
    this.reconnect = { since: Date.now(), timer: setTimeout(() => void this.restartIce(), 0) };
  }

  /** Reinicia o ICE (nova oferta com credenciais novas) até reconectar. */
  private async restartIce() {
    if (this.closed || !this.reconnect) return;
    if (Date.now() - this.reconnect.since > RECONNECT_GIVE_UP_MS) {
      this.reconnect = null;
      this.gaveUp = true;
      this.events.onState?.("failed");
      return;
    }
    try {
      const offer = await this.pc.createOffer({ iceRestart: true });
      await this.pc.setLocalDescription(offer);
      this.events.onSignal({ kind: "offer", sdp: offer.sdp ?? "" });
    } catch (err) {
      console.warn("reinício de ICE falhou", err);
    }
    if (this.reconnect) {
      this.reconnect.timer = setTimeout(() => void this.restartIce(), RESTART_RETRY_MS);
    }
  }

  /**
   * A sinalização voltou: se está reconectando, manda a oferta nova já, sem
   * esperar a próxima tentativa (as anteriores se perderam com a rede fora).
   */
  signalingOnline() {
    if (this.closed || !this.reconnect) return;
    clearTimeout(this.reconnect.timer);
    this.reconnect.timer = setTimeout(() => void this.restartIce(), 0);
  }

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
      const ice = this.pc.iceConnectionState;
      if (this.reconnect && (ice === "connected" || ice === "completed")) this.finishReconnect();
    } else if (payload.kind === "candidate") {
      if (this.pc.remoteDescription) await this.pc.addIceCandidate(payload.candidate);
      else this.pendingCandidates.push(payload.candidate);
    }
  }

  /** Pede um keyframe ao host (ex.: quando o `<video>` é montado de novo). */
  requestKeyframe() {
    if (this.control.readyState === "open") this.control.send(JSON.stringify({ t: "keyframe" }));
  }

  /** Pede ao host para transmitir outro monitor. */
  selectMonitor(index: number) {
    if (this.control.readyState === "open") {
      this.control.send(JSON.stringify({ t: "select_monitor", index }));
    }
  }

  /** Fingerprints (nosso, do host) vistos nas descrições SDP. */
  private fingerprints(): { local: string; remote: string } | null {
    const local = sdpFingerprint(this.pc.localDescription?.sdp);
    const remote = sdpFingerprint(this.pc.remoteDescription?.sdp);
    return local && remote ? { local, remote } : null;
  }

  private failAuth(outcome: VerifyOutcome) {
    if (this.authDone) return;
    this.authDone = true;
    this.authenticated = false;
    this.events.onAuth?.(outcome);
  }

  /** Envia nossa prova e aguarda a do host. */
  private async startAuth() {
    if (!this.auth) {
      // Sem autenticação configurada (testes de transporte): segue sem ela.
      this.authenticated = true;
      this.authDone = true;
      return;
    }
    const fps = this.fingerprints();
    if (!fps) {
      this.failAuth({ result: "failed", reason: "conexão sem fingerprint DTLS" });
      return;
    }
    this.events.onVerificationCode?.(
      await verificationCode(this.auth.requestId, fps.local, fps.remote),
    );
    try {
      const proof = await this.auth.crypto.sign(
        sessionMessage(this.auth.requestId, "viewer", fps.local, fps.remote),
      );
      if (this.control.readyState === "open") {
        this.control.send(JSON.stringify({ t: "auth", pk: proof.publicKey, sig: proof.signature }));
      }
    } catch (err) {
      this.failAuth({ result: "failed", reason: `não foi possível assinar: ${String(err)}` });
      return;
    }
    this.timers.push(
      setTimeout(() => {
        if (!this.authDone) {
          this.failAuth({
            result: "failed",
            reason: "o outro computador não comprovou a identidade",
          });
        }
      }, AUTH_TIMEOUT_MS) as unknown as ReturnType<typeof setInterval>,
    );
  }

  private async verifyHost(pk: string, sig: string) {
    if (!this.auth || this.authDone) return;
    const fps = this.fingerprints();
    if (!fps) return;
    let outcome: VerifyOutcome;
    try {
      outcome = await this.auth.crypto.verify(
        pk,
        sig,
        sessionMessage(this.auth.requestId, "host", fps.remote, fps.local),
      );
    } catch (err) {
      outcome = { result: "failed", reason: String(err) };
    }
    if (outcome.result === "ok") {
      this.authenticated = true;
      this.authDone = true;
      this.events.onAuth?.(outcome);
      this.timers.push(setInterval(() => void this.sendRxReport(), RX_REPORT_MS));
    } else {
      this.failAuth(outcome);
    }
  }

  /** Pacotes recebidos/perdidos desde o último relatório. */
  private async sendRxReport() {
    if (this.control.readyState !== "open") return;
    let lost = 0;
    let received = 0;
    (await this.pc.getStats()).forEach(
      (s: { type: string; kind?: string; packetsLost?: number; packetsReceived?: number }) => {
        if (s.type === "inbound-rtp" && s.kind === "video") {
          lost = s.packetsLost ?? 0;
          received = s.packetsReceived ?? 0;
        }
      },
    );
    const prev = this.lastRx ?? { lost: 0, received: 0 };
    this.lastRx = { lost, received };
    const report = {
      lost: Math.max(0, lost - prev.lost),
      received: Math.max(0, received - prev.received),
    };
    this.control.send(JSON.stringify({ t: "rx_report", ...report, rtt_ms: this.lastRttMs }));
    this.events.onRxReport?.(report.lost, report.received);
  }

  /** Manda texto copiado aqui para o host (só com controle permitido). */
  sendClipboard(text: string): boolean {
    if (!this.authenticated || !this.controlAllowed) return false;
    if (this.control.readyState !== "open" || text.length > MAX_CLIPBOARD_CHARS) return false;
    this.control.send(JSON.stringify({ t: "clipboard", text }));
    return true;
  }

  /** O host foi verificado (vídeo e entrada liberados). */
  get isAuthenticated(): boolean {
    return this.authenticated;
  }

  /** Envia entrada ao host. Movimentos vão pelo canal não confiável. */
  sendInput(msg: InputMessage): boolean {
    // Nada de teclas (senhas!) para um host não verificado ou sem permissão.
    if (!this.authenticated) return false;
    if (!this.controlAllowed && msg.t !== "release_all") return false;
    const channel =
      msg.t === "mouse_move" && this.input.readyState === "open" ? this.input : this.control;
    if (channel.readyState !== "open") return false;
    // Evita acumular movimentos se a rede engasgar.
    if (msg.t === "mouse_move" && channel.bufferedAmount > 16 * 1024) return false;
    channel.send(JSON.stringify(msg));
    return true;
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
      this.lastRttMs = Math.round(performance.now() - this.inFlight.at);
      this.events.onRtt?.(this.lastRttMs);
      this.inFlight = null;
    } else if (msg.t === "cursor_shape" || msg.t === "cursor_visible") {
      this.events.onCursor?.(msg);
    } else if (msg.t === "monitors") {
      this.events.onMonitors?.(msg.list, msg.active);
    } else if (msg.t === "auth") {
      // O SDP já está aplicado quando o canal abre: dá para verificar já.
      void this.verifyHost(msg.pk, msg.sig);
    } else if (msg.t === "clipboard") {
      if (this.authenticated && this.controlAllowed) this.events.onClipboard?.(msg.text);
    } else if (msg.t === "permissions") {
      this.controlAllowed = msg.control;
      this.events.onPermissions?.(msg.control);
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
    this.timers.splice(0).forEach((t) => clearInterval(t));
  }

  close() {
    if (this.closed) return;
    this.closed = true;
    this.stopTimers();
    if (this.reconnect) clearTimeout(this.reconnect.timer);
    this.reconnect = null;
    if (this.control.readyState === "open") this.control.send(JSON.stringify({ t: "release_all" }));
    this.input.close();
    this.control.close();
    this.pc.close();
  }
}

type CodecCapability = NonNullable<
  ReturnType<typeof RTCRtpReceiver.getCapabilities>
>["codecs"][number];

/** Coloca H.264 constrained baseline (`42e01f`) no topo da oferta. */
function preferH264(transceiver: RTCRtpTransceiver) {
  const codecs = RTCRtpReceiver.getCapabilities("video")?.codecs ?? [];
  const score = (c: CodecCapability) => {
    if (c.mimeType.toLowerCase() !== "video/h264") return 2;
    const fmtp = c.sdpFmtpLine ?? "";
    return fmtp.includes("profile-level-id=42e01f") && fmtp.includes("packetization-mode=1")
      ? 0
      : 1;
  };
  if (!codecs.some((c) => score(c) === 0)) return; // sem H.264: deixa o padrão
  try {
    transceiver.setCodecPreferences([...codecs].sort((a, b) => score(a) - score(b)));
  } catch {
    // Navegadores antigos: segue sem preferência.
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
