import type { IceServerDto, PeerState, RouteInfo } from "./viewer-peer";

/**
 * Estado da sinalização na interface, como um reducer puro (testável sem DOM).
 * Os tipos espelham `kr_proto::ServerMessage` e o `StatusDto` do Rust.
 */

export type ConnectionStatus =
  | { state: "connecting" }
  | { state: "online"; deviceId: string }
  | { state: "offline"; reason: string; retryInMs: number };

export type RejectReason = "declined" | "timeout";

export type ErrorCode =
  | "bad_message"
  | "unsupported_version"
  | "auth_required"
  | "auth_failed"
  | "target_offline"
  | "self_connect"
  | "rate_limited"
  | "unknown_request"
  | "internal";

/** Mensagens do servidor que a interface trata (o restante é ignorado). */
export type ServerMessage =
  | {
      type: "incoming_request";
      request_id: string;
      from_id: string;
      from_name: string;
      from_public_key: string;
    }
  | { type: "connect_pending"; request_id: string }
  | {
      type: "connect_accepted" | "session_ready";
      request_id: string;
      peer_id: string;
      peer_public_key: string;
      ice_servers?: IceServerDto[];
    }
  | { type: "connect_rejected"; request_id: string; reason: RejectReason }
  | { type: "session_ended"; request_id: string }
  | { type: "error"; code: ErrorCode; message: string; request_id?: string }
  | { type: "signal"; request_id: string; from_id?: string; payload?: unknown };

/** Estado da conexão ponto a ponto da sessão ativa. */
export interface P2pStatus {
  state: PeerState;
  rttMs: number | null;
  route: RouteInfo | null;
}

export interface IncomingRequest {
  requestId: string;
  fromId: string;
  fromName: string;
  receivedAt: number;
}

export type Outgoing =
  | { kind: "idle" }
  | { kind: "sending"; targetId: string }
  | { kind: "pending"; requestId: string; targetId: string }
  | { kind: "accepted"; requestId: string; peerId: string }
  | { kind: "failed"; message: string };

export interface ActiveSession {
  requestId: string;
  peerId: string;
  /** Este computador está sendo acessado (host) ou acessando (viewer). */
  role: "host" | "viewer";
  p2p: P2pStatus;
}

const P2P_INITIAL: P2pStatus = { state: "connecting", rttMs: null, route: null };

export interface SignalingState {
  status: ConnectionStatus;
  /** Último ID recebido; continua visível quando a conexão cai (o ID não muda). */
  deviceId: string | null;
  outgoing: Outgoing;
  /** Pedidos recebidos aguardando decisão (o primeiro é exibido). */
  incoming: IncomingRequest[];
  active: ActiveSession | null;
  /** Aviso informativo de curta duração (ex.: "O outro lado encerrou"). */
  notice: string | null;
}

export const initialState: SignalingState = {
  status: { state: "connecting" },
  deviceId: null,
  outgoing: { kind: "idle" },
  incoming: [],
  active: null,
  notice: null,
};

export type Action =
  | { type: "status"; status: ConnectionStatus }
  | { type: "message"; msg: ServerMessage; now: number }
  | { type: "connect_started"; targetId: string }
  | { type: "connect_sent"; requestId: string; targetId: string }
  | { type: "connect_failed"; message: string }
  | { type: "incoming_answered"; requestId: string; accepted: boolean }
  | { type: "outgoing_reset" }
  | { type: "session_closed"; requestId: string }
  | { type: "notice_dismissed" }
  | {
      type: "p2p_status";
      requestId: string;
      state?: PeerState;
      rttMs?: number | null;
      route?: RouteInfo | null;
    };

const ERROR_TEXT: Partial<Record<ErrorCode, string>> = {
  target_offline: "Computador offline ou ID inexistente.",
  self_connect: "Esse é o ID deste computador.",
  rate_limited: "Muitas tentativas. Aguarde um minuto e tente de novo.",
  unknown_request: "O pedido expirou.",
};

export function errorText(code: ErrorCode, fallback: string): string {
  return ERROR_TEXT[code] ?? fallback;
}

function outgoingRequestId(o: Outgoing): string | null {
  return o.kind === "pending" || o.kind === "accepted" ? o.requestId : null;
}

export function reducer(state: SignalingState, action: Action): SignalingState {
  switch (action.type) {
    case "status": {
      // Ao perder a conexão, um pedido em andamento não tem mais como avançar.
      const lost = action.status.state !== "online" && state.outgoing.kind === "pending";
      return {
        ...state,
        status: action.status,
        deviceId: action.status.state === "online" ? action.status.deviceId : state.deviceId,
        ...(lost
          ? { outgoing: { kind: "failed", message: "Conexão com o servidor perdida." } }
          : {}),
      };
    }
    case "connect_started":
      return { ...state, outgoing: { kind: "sending", targetId: action.targetId }, notice: null };
    case "connect_sent":
      return {
        ...state,
        outgoing: { kind: "pending", requestId: action.requestId, targetId: action.targetId },
      };
    case "connect_failed":
      return { ...state, outgoing: { kind: "failed", message: action.message } };
    case "outgoing_reset":
      return { ...state, outgoing: { kind: "idle" } };
    case "incoming_answered":
      return {
        ...state,
        incoming: state.incoming.filter((r) => r.requestId !== action.requestId),
      };
    case "session_closed":
      return {
        ...state,
        active: state.active?.requestId === action.requestId ? null : state.active,
        outgoing:
          outgoingRequestId(state.outgoing) === action.requestId
            ? { kind: "idle" }
            : state.outgoing,
      };
    case "notice_dismissed":
      return { ...state, notice: null };
    case "p2p_status": {
      if (state.active?.requestId !== action.requestId) return state;
      const p2p = { ...state.active.p2p };
      if (action.state !== undefined) p2p.state = action.state;
      if (action.rttMs !== undefined) p2p.rttMs = action.rttMs;
      if (action.route !== undefined) p2p.route = action.route;
      return { ...state, active: { ...state.active, p2p } };
    }
    case "message":
      return onMessage(state, action.msg, action.now);
  }
}

function onMessage(state: SignalingState, msg: ServerMessage, now: number): SignalingState {
  const mine = outgoingRequestId(state.outgoing) === msg.request_id;

  switch (msg.type) {
    case "incoming_request":
      if (state.incoming.some((r) => r.requestId === msg.request_id)) return state;
      return {
        ...state,
        incoming: [
          ...state.incoming,
          {
            requestId: msg.request_id,
            fromId: msg.from_id,
            fromName: msg.from_name,
            receivedAt: now,
          },
        ],
      };

    case "connect_pending":
      return state;

    case "connect_accepted":
      if (!mine) return state;
      return {
        ...state,
        outgoing: { kind: "accepted", requestId: msg.request_id, peerId: msg.peer_id },
        active: {
          requestId: msg.request_id,
          peerId: msg.peer_id,
          role: "viewer",
          p2p: P2P_INITIAL,
        },
      };

    case "session_ready":
      return {
        ...state,
        active: { requestId: msg.request_id, peerId: msg.peer_id, role: "host", p2p: P2P_INITIAL },
      };

    case "connect_rejected":
      if (!mine) return state;
      return {
        ...state,
        outgoing: {
          kind: "failed",
          message:
            msg.reason === "declined"
              ? "O outro computador recusou o acesso."
              : "Sem resposta do outro computador.",
        },
      };

    case "session_ended": {
      const wasIncoming = state.incoming.some((r) => r.requestId === msg.request_id);
      const wasActive = state.active?.requestId === msg.request_id;
      return {
        ...state,
        incoming: state.incoming.filter((r) => r.requestId !== msg.request_id),
        active: wasActive ? null : state.active,
        outgoing: mine
          ? state.outgoing.kind === "pending"
            ? { kind: "failed", message: "O pedido foi cancelado." }
            : { kind: "idle" }
          : state.outgoing,
        notice: wasActive
          ? "O outro computador encerrou a sessão."
          : wasIncoming
            ? "O pedido de acesso expirou ou foi cancelado."
            : state.notice,
      };
    }

    case "error":
      if (msg.request_id && mine) {
        return {
          ...state,
          outgoing: { kind: "failed", message: errorText(msg.code, msg.message) },
        };
      }
      if (
        msg.request_id &&
        state.outgoing.kind === "sending" &&
        (msg.code === "target_offline" ||
          msg.code === "self_connect" ||
          msg.code === "rate_limited")
      ) {
        // Erro chegou antes da confirmação local do envio.
        return {
          ...state,
          outgoing: { kind: "failed", message: errorText(msg.code, msg.message) },
        };
      }
      return state;

    case "signal":
      return state;
  }
}

/** "123456789" → "123 456 789". */
export function formatId(id: string): string {
  return id.replace(/(\d{3})(?=\d)/g, "$1 ");
}

/** Mantém só dígitos (até 9). */
export function sanitizeIdInput(raw: string): string {
  return raw.replace(/\D/g, "").slice(0, 9);
}
