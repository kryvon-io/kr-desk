/**
 * Espelho TypeScript de `crates/kr-proto` (fonte da verdade do protocolo).
 * Os fixtures em `crates/kr-proto/fixtures/` são validados pelos testes dos
 * dois lados para garantir compatibilidade.
 */
import { z } from "zod";

export const PROTOCOL_VERSION = 1;
export const AUTH_CONTEXT = "kr-desk/auth/v1:";

/** ID público de dispositivo: 9 dígitos, sem zero à esquerda. */
export const DEVICE_ID_RE = /^[1-9]\d{8}$/;
const UUID_RE = /^[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/;
const B64URL_RE = /^[A-Za-z0-9_-]+$/;

const requestId = z.string().regex(UUID_RE, "request_id deve ser UUID v4 minúsculo");
const deviceId = z.string().regex(DEVICE_ID_RE, "ID deve ter 9 dígitos");

export const clientMessageSchema = z.discriminatedUnion("type", [
  z.object({
    type: z.literal("auth"),
    public_key: z.string().length(43).regex(B64URL_RE),
    signature: z.string().length(86).regex(B64URL_RE),
    device_name: z.string().trim().min(1).max(64),
    app_version: z.string().min(1).max(32),
  }),
  z.object({ type: z.literal("connect_request"), request_id: requestId, target_id: deviceId }),
  z.object({ type: z.literal("connect_response"), request_id: requestId, accept: z.boolean() }),
  z.object({
    type: z.literal("signal"),
    request_id: requestId,
    payload: z.record(z.string(), z.unknown()),
  }),
  z.object({ type: z.literal("end_session"), request_id: requestId }),
  z.object({ type: z.literal("ping"), nonce: z.number().int().nonnegative() }),
]);

export const envelopeSchema = z.looseObject({ v: z.number().int() });

export type ClientMessage = z.infer<typeof clientMessageSchema>;

export interface IceServer {
  urls: string[];
  username?: string;
  credential?: string;
}

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

export type ServerMessage =
  | { type: "hello"; nonce: string; server_time_ms: number }
  | { type: "auth_ok"; device_id: string }
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
      ice_servers: IceServer[];
    }
  | { type: "connect_rejected"; request_id: string; reason: RejectReason }
  | { type: "signal"; request_id: string; from_id: string; payload: Record<string, unknown> }
  | { type: "session_ended"; request_id: string }
  | { type: "pong"; nonce: number }
  | { type: "reconnect_soon"; in_ms: number }
  | { type: "error"; code: ErrorCode; message: string; request_id?: string };

/** Serializa com o envelope versionado. */
export function encode(msg: ServerMessage): string {
  return JSON.stringify({ v: PROTOCOL_VERSION, ...msg });
}

export type DecodeResult =
  | { ok: true; msg: ClientMessage }
  | { ok: false; code: "bad_message" | "unsupported_version"; message: string };

/** Valida uma mensagem recebida do cliente. Nunca lança exceção. */
export function decode(raw: string): DecodeResult {
  let json: unknown;
  try {
    json = JSON.parse(raw);
  } catch {
    return { ok: false, code: "bad_message", message: "JSON inválido" };
  }
  const env = envelopeSchema.safeParse(json);
  if (!env.success) return { ok: false, code: "bad_message", message: "Envelope inválido" };
  if (env.data.v !== PROTOCOL_VERSION) {
    return {
      ok: false,
      code: "unsupported_version",
      message: `Versão ${env.data.v} não suportada (servidor: ${PROTOCOL_VERSION})`,
    };
  }
  const parsed = clientMessageSchema.safeParse(json);
  if (!parsed.success) {
    return { ok: false, code: "bad_message", message: parsed.error.issues[0]?.message ?? "" };
  }
  return { ok: true, msg: parsed.data };
}
