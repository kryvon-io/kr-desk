/** Espelho de `kr_proto::control` (DataChannel `control`). */

export const CONTROL_LABEL = "control";

export type CandidateType = "host" | "srflx" | "prflx" | "relay";

export type ControlMessage =
  | { t: "ping"; n: number }
  | { t: "pong"; n: number }
  | { t: "route"; local: CandidateType; remote: CandidateType; protocol: string }
  | { t: "keyframe" };

const CANDIDATE_TYPES: readonly string[] = ["host", "srflx", "prflx", "relay"];

export function isCandidateType(v: unknown): v is CandidateType {
  return typeof v === "string" && CANDIDATE_TYPES.includes(v);
}

/** Valida uma mensagem recebida; retorna null se não for reconhecida. */
export function parseControl(raw: string): ControlMessage | null {
  let v: unknown;
  try {
    v = JSON.parse(raw);
  } catch {
    return null;
  }
  if (typeof v !== "object" || v === null) return null;
  const o = v as Record<string, unknown>;
  if ((o.t === "ping" || o.t === "pong") && typeof o.n === "number") return { t: o.t, n: o.n };
  if (o.t === "route" && isCandidateType(o.local) && isCandidateType(o.remote)) {
    return { t: "route", local: o.local, remote: o.remote, protocol: String(o.protocol ?? "") };
  }
  if (o.t === "keyframe") return { t: "keyframe" };
  return null;
}
