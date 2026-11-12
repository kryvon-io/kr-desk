/**
 * Lógica de sinalização: autenticação, presença, pedidos de conexão e repasse
 * de mensagens WebRTC. Cada WebSocket vira uma `Connection`. Nada de estado de
 * roteamento fica em memória entre conexões: tudo passa pelo `Bus`, porque os
 * dois lados de uma sessão podem estar em instâncias diferentes.
 */
import { randomUUID } from "node:crypto";

import type { WebSocket } from "ws";

import type { Bus, Unsubscribe } from "./bus.js";
import { newDeviceId, newNonce, verifyAuthSignature } from "./identity.js";
import {
  decode,
  encode,
  type ClientMessage,
  type ErrorCode,
  type ServerMessage,
} from "./protocol.js";
import type { DeviceStore } from "./store.js";
import type { TurnProvider } from "./turn.js";

export interface HubConfig {
  /** Tempo máximo de vida de uma conexão (limite da função na Vercel). */
  maxLifetimeMs: number;
  /** Aviso `reconnect_soon` enviado este tempo antes do fechamento. */
  reconnectNoticeMs: number;
  authTimeoutMs: number;
  /** Tempo que o host tem para responder a um pedido. */
  pendingTimeoutMs: number;
  /** TTL da presença; o cliente envia `ping` bem antes disso. */
  presenceTtlSec: number;
}

export const DEFAULT_HUB_CONFIG: HubConfig = {
  maxLifetimeMs: 270_000,
  reconnectNoticeMs: 30_000,
  authTimeoutMs: 10_000,
  pendingTimeoutMs: 45_000,
  presenceTtlSec: 60,
};

export interface HubLogger {
  info(obj: object, msg: string): void;
  warn(obj: object, msg: string): void;
  error(obj: object, msg: string): void;
}

export interface HubDeps {
  bus: Bus;
  store: DeviceStore;
  turn: TurnProvider;
  log: HubLogger;
  config?: Partial<HubConfig>;
}

/** Limites de taxa: [máximo, janela em segundos]. */
const LIMITS = {
  authPerIp: [30, 60],
  registerPerIp: [10, 3600],
  connectPerDevice: [10, 60],
} as const satisfies Record<string, readonly [number, number]>;

const MAX_MESSAGES_PER_10S = 600;
const MAX_BAD_MESSAGES = 10;
const ACTIVE_SESSION_TTL_SEC = 12 * 60 * 60;

/** Pedido/sessão guardado no bus em `req:{request_id}`. */
interface SessionRecord {
  viewerId: string;
  viewerName: string;
  viewerKey: string;
  hostId: string;
  status: "pending" | "active";
}

/** Mensagem trafegando no canal `dev:{deviceId}`. */
type Routed = { kind: "deliver"; msg: ServerMessage } | { kind: "superseded"; connId: string };

const reqKey = (id: string) => `req:${id}`;
const presenceKey = (id: string) => `presence:${id}`;
const deviceChannel = (id: string) => `dev:${id}`;

// Códigos de fechamento (4000–4999 são livres para a aplicação).
export const CLOSE_ROTATE = 4000;
export const CLOSE_AUTH_TIMEOUT = 4001;
export const CLOSE_POLICY = 4008;

export class SignalingHub {
  readonly config: HubConfig;
  private readonly connections = new Set<Connection>();

  constructor(readonly deps: HubDeps) {
    this.config = { ...DEFAULT_HUB_CONFIG, ...deps.config };
  }

  attach(socket: WebSocket, ip: string) {
    const conn = new Connection(this, socket, ip);
    this.connections.add(conn);
    socket.once("close", () => this.connections.delete(conn));
    conn.start();
    return conn;
  }

  get size() {
    return this.connections.size;
  }

  async closeAll() {
    for (const c of this.connections) c.socket.close(1001, "servidor encerrando");
  }

  /** Incrementa um contador e diz se passou do limite. */
  async limited(key: string, [max, windowSec]: readonly [number, number]) {
    return (await this.deps.bus.incr(`rl:${key}`, windowSec)) > max;
  }

  async route(deviceId: string, msg: ServerMessage) {
    const routed: Routed = { kind: "deliver", msg };
    await this.deps.bus.publish(deviceChannel(deviceId), JSON.stringify(routed));
  }

  async loadSession(requestId: string): Promise<SessionRecord | null> {
    const raw = await this.deps.bus.get(reqKey(requestId));
    return raw ? (JSON.parse(raw) as SessionRecord) : null;
  }

  async saveSession(requestId: string, rec: SessionRecord, ttlSec: number) {
    await this.deps.bus.set(reqKey(requestId), JSON.stringify(rec), ttlSec);
  }

  async deleteSession(requestId: string) {
    await this.deps.bus.del(reqKey(requestId));
  }

  /** Grava auditoria sem bloquear nem derrubar a sinalização. */
  audit(fn: () => Promise<void>) {
    fn().catch((err: unknown) => this.deps.log.warn({ err: String(err) }, "falha na auditoria"));
  }
}

class Connection {
  readonly id = randomUUID();
  private readonly nonce = newNonce();
  private device: { id: string; name: string; publicKey: string } | null = null;
  private unsubscribe: Unsubscribe | null = null;
  private superseded = false;
  private closed = false;
  private timers: NodeJS.Timeout[] = [];
  private windowStart = Date.now();
  private windowCount = 0;
  private badMessages = 0;
  /** Serializa o processamento para preservar a ordem das mensagens. */
  private queue: Promise<void> = Promise.resolve();

  constructor(
    private readonly hub: SignalingHub,
    readonly socket: WebSocket,
    private readonly ip: string,
  ) {}

  private get log() {
    return this.hub.deps.log;
  }

  start() {
    const { config } = this.hub;
    this.send({ type: "hello", nonce: this.nonce, server_time_ms: Date.now() });

    this.later(config.authTimeoutMs, () => {
      if (!this.device) this.socket.close(CLOSE_AUTH_TIMEOUT, "auth timeout");
    });
    this.later(Math.max(0, config.maxLifetimeMs - config.reconnectNoticeMs), () =>
      this.send({ type: "reconnect_soon", in_ms: config.reconnectNoticeMs }),
    );
    this.later(config.maxLifetimeMs, () => this.socket.close(CLOSE_ROTATE, "rotate"));

    this.socket.on("message", (data, isBinary) => {
      if (isBinary) return this.fail("bad_message", "Somente mensagens de texto");
      const raw = data.toString();
      this.queue = this.queue
        .then(() => this.onMessage(raw))
        .catch((err: unknown) => {
          this.log.error({ err: String(err), conn: this.id }, "erro ao processar mensagem");
          this.send({ type: "error", code: "internal", message: "Erro interno" });
        });
    });
    this.socket.on("close", () => void this.onClose());
    this.socket.on("error", (err) => this.log.warn({ err: String(err) }, "erro no socket"));
  }

  private later(ms: number, fn: () => void) {
    const t = setTimeout(fn, ms);
    t.unref?.();
    this.timers.push(t);
  }

  private send(msg: ServerMessage) {
    if (this.socket.readyState === this.socket.OPEN) this.socket.send(encode(msg));
  }

  private fail(code: ErrorCode, message: string, requestId?: string) {
    this.send({ type: "error", code, message, ...(requestId ? { request_id: requestId } : {}) });
  }

  private async onMessage(raw: string) {
    if (this.closed) return;

    const now = Date.now();
    if (now - this.windowStart > 10_000) {
      this.windowStart = now;
      this.windowCount = 0;
    }
    if (++this.windowCount > MAX_MESSAGES_PER_10S) {
      this.fail("rate_limited", "Mensagens demais");
      this.socket.close(CLOSE_POLICY, "rate limit");
      return;
    }

    const result = decode(raw);
    if (!result.ok) {
      this.fail(result.code, result.message);
      if (++this.badMessages >= MAX_BAD_MESSAGES) this.socket.close(CLOSE_POLICY, "bad messages");
      return;
    }

    const msg = result.msg;
    if (msg.type === "ping") {
      await this.refreshPresence();
      this.send({ type: "pong", nonce: msg.nonce });
      return;
    }
    if (msg.type === "auth") return this.onAuth(msg);
    if (!this.device) return this.fail("auth_required", "Autentique-se primeiro");

    switch (msg.type) {
      case "connect_request":
        return this.onConnectRequest(msg);
      case "connect_response":
        return this.onConnectResponse(msg);
      case "signal":
        return this.onSignal(msg);
      case "end_session":
        return this.onEndSession(msg);
    }
  }

  // --- autenticação --------------------------------------------------------

  private async onAuth(msg: Extract<ClientMessage, { type: "auth" }>) {
    if (this.device) return this.fail("bad_message", "Já autenticado");
    const { store } = this.hub.deps;

    if (await this.hub.limited(`auth:${this.ip}`, LIMITS.authPerIp)) {
      return this.fail("rate_limited", "Muitas tentativas de login");
    }
    if (!verifyAuthSignature(msg.public_key, this.nonce, msg.signature)) {
      this.log.warn({ ip: this.ip }, "assinatura inválida");
      return this.fail("auth_failed", "Assinatura inválida");
    }

    let device = await store.findByPublicKey(msg.public_key);
    if (device) {
      await store.touch(device.id, msg.device_name, msg.app_version);
    } else {
      if (await this.hub.limited(`register:${this.ip}`, LIMITS.registerPerIp)) {
        return this.fail("rate_limited", "Muitos registros a partir deste endereço");
      }
      for (let attempt = 0; attempt < 5 && !device; attempt++) {
        device = await store.create({
          id: newDeviceId(),
          publicKey: msg.public_key,
          name: msg.device_name,
          appVersion: msg.app_version,
        });
      }
      if (!device) return this.fail("internal", "Não foi possível alocar um ID");
      this.log.info({ deviceId: device.id }, "dispositivo registrado");
    }
    if (this.closed) return;

    this.device = { id: device.id, name: msg.device_name, publicKey: msg.public_key };
    const { bus } = this.hub.deps;
    this.unsubscribe = await bus.subscribe(deviceChannel(device.id), (raw) => this.onRouted(raw));
    // Conexões antigas do mesmo dispositivo param de receber (sem duplicar mensagens).
    await bus.publish(
      deviceChannel(device.id),
      JSON.stringify({ kind: "superseded", connId: this.id } satisfies Routed),
    );
    await this.refreshPresence();
    this.send({ type: "auth_ok", device_id: device.id });
  }

  private onRouted(raw: string) {
    const routed = JSON.parse(raw) as Routed;
    if (routed.kind === "superseded") {
      if (routed.connId !== this.id && !this.superseded) {
        this.superseded = true;
        void this.unsubscribe?.();
        this.unsubscribe = null;
      }
      return;
    }
    if (!this.superseded) this.send(routed.msg);
  }

  private async refreshPresence() {
    if (!this.device || this.superseded) return;
    const { bus } = this.hub.deps;
    await bus.set(presenceKey(this.device.id), this.id, this.hub.config.presenceTtlSec);
  }

  // --- pedidos de conexão --------------------------------------------------

  private async onConnectRequest(msg: Extract<ClientMessage, { type: "connect_request" }>) {
    const me = this.device!;
    const rid = msg.request_id;

    if (msg.target_id === me.id)
      return this.fail("self_connect", "Não é possível conectar a si mesmo", rid);
    if (await this.hub.limited(`connect:${me.id}`, LIMITS.connectPerDevice)) {
      return this.fail("rate_limited", "Muitos pedidos de conexão; aguarde um minuto", rid);
    }
    if (await this.hub.loadSession(rid))
      return this.fail("bad_message", "request_id já usado", rid);

    const online = await this.hub.deps.bus.get(presenceKey(msg.target_id));
    if (!online) return this.fail("target_offline", "Dispositivo offline ou inexistente", rid);

    const pendingTtl = Math.ceil(this.hub.config.pendingTimeoutMs / 1000) + 5;
    await this.hub.saveSession(
      rid,
      {
        viewerId: me.id,
        viewerName: me.name,
        viewerKey: me.publicKey,
        hostId: msg.target_id,
        status: "pending",
      },
      pendingTtl,
    );
    await this.hub.route(msg.target_id, {
      type: "incoming_request",
      request_id: rid,
      from_id: me.id,
      from_name: me.name,
      from_public_key: me.publicKey,
      ...(msg.unattended === true ? { unattended: true as const } : {}),
    });
    this.send({ type: "connect_pending", request_id: rid });
    this.hub.audit(() => this.hub.deps.store.logSession(rid, msg.target_id, me.id));

    // Sem resposta do host a tempo → recusa por timeout.
    this.later(this.hub.config.pendingTimeoutMs, () => {
      void (async () => {
        const rec = await this.hub.loadSession(rid);
        if (rec?.status !== "pending") return;
        await this.hub.deleteSession(rid);
        await this.hub.route(rec.viewerId, {
          type: "connect_rejected",
          request_id: rid,
          reason: "timeout",
        });
        await this.hub.route(rec.hostId, { type: "session_ended", request_id: rid });
        this.hub.audit(() => this.hub.deps.store.updateSession(rid, "rejected", "timeout"));
      })().catch((err: unknown) =>
        this.log.warn({ err: String(err) }, "falha no timeout do pedido"),
      );
    });
  }

  private async onConnectResponse(msg: Extract<ClientMessage, { type: "connect_response" }>) {
    const me = this.device!;
    const rid = msg.request_id;
    const rec = await this.hub.loadSession(rid);
    if (!rec || rec.hostId !== me.id || rec.status !== "pending") {
      return this.fail("unknown_request", "Pedido inexistente ou expirado", rid);
    }

    if (!msg.accept) {
      await this.hub.deleteSession(rid);
      await this.hub.route(rec.viewerId, {
        type: "connect_rejected",
        request_id: rid,
        reason: "declined",
      });
      this.hub.audit(() => this.hub.deps.store.updateSession(rid, "rejected", "declined"));
      return;
    }

    await this.hub.saveSession(rid, { ...rec, status: "active" }, ACTIVE_SESSION_TTL_SEC);
    const iceServers = await this.hub.deps.turn.iceServers();
    await this.hub.route(rec.viewerId, {
      type: "connect_accepted",
      request_id: rid,
      peer_id: me.id,
      peer_public_key: me.publicKey,
      ice_servers: iceServers,
    });
    this.send({
      type: "session_ready",
      request_id: rid,
      peer_id: rec.viewerId,
      peer_public_key: rec.viewerKey,
      ice_servers: iceServers,
    });
    this.hub.audit(() => this.hub.deps.store.updateSession(rid, "accepted"));
  }

  // --- sessão ativa --------------------------------------------------------

  /** Retorna o outro participante, ou null se este dispositivo não faz parte. */
  private peerOf(rec: SessionRecord): string | null {
    const me = this.device!.id;
    if (rec.viewerId === me) return rec.hostId;
    if (rec.hostId === me) return rec.viewerId;
    return null;
  }

  private async onSignal(msg: Extract<ClientMessage, { type: "signal" }>) {
    const rec = await this.hub.loadSession(msg.request_id);
    const peer = rec && rec.status === "active" ? this.peerOf(rec) : null;
    if (!peer) return this.fail("unknown_request", "Sessão inexistente", msg.request_id);
    await this.hub.route(peer, {
      type: "signal",
      request_id: msg.request_id,
      from_id: this.device!.id,
      payload: msg.payload,
    });
  }

  private async onEndSession(msg: Extract<ClientMessage, { type: "end_session" }>) {
    const rec = await this.hub.loadSession(msg.request_id);
    const peer = rec ? this.peerOf(rec) : null;
    if (!rec || !peer) return this.fail("unknown_request", "Sessão inexistente", msg.request_id);
    await this.hub.deleteSession(msg.request_id);
    await this.hub.route(peer, { type: "session_ended", request_id: msg.request_id });
    const status = rec.status === "pending" ? "rejected" : "ended";
    const reason = rec.status === "pending" ? "cancelled" : "ended_by_peer";
    this.hub.audit(() => this.hub.deps.store.updateSession(msg.request_id, status, reason));
  }

  // --- encerramento --------------------------------------------------------

  private async onClose() {
    if (this.closed) return;
    this.closed = true;
    for (const t of this.timers) clearTimeout(t);
    this.timers = [];
    try {
      await this.unsubscribe?.();
      if (this.device && !this.superseded) {
        const { bus } = this.hub.deps;
        // Só remove a presença se ela ainda for desta conexão.
        if ((await bus.get(presenceKey(this.device.id))) === this.id) {
          await bus.del(presenceKey(this.device.id));
        }
      }
    } catch (err) {
      this.log.warn({ err: String(err) }, "falha ao limpar conexão");
    }
  }
}
