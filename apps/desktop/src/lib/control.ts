/** Espelho de `kr_proto::control` (DataChannel `control`). */

export const CONTROL_LABEL = "control";
/** Canal não confiável para movimentos do mouse. */
export const INPUT_LABEL = "input";

export type CandidateType = "host" | "srflx" | "prflx" | "relay";

export type ControlMessage =
  | { t: "ping"; n: number }
  | { t: "pong"; n: number }
  | { t: "route"; local: CandidateType; remote: CandidateType; protocol: string }
  | { t: "keyframe" }
  | { t: "cursor_shape"; w: number; h: number; hx: number; hy: number; rgba: string }
  | { t: "cursor_visible"; visible: boolean }
  | { t: "monitors"; list: MonitorInfo[]; active: number };

/** Monitor do host disponível para transmissão. */
export interface MonitorInfo {
  index: number;
  name: string;
  width: number;
  height: number;
  primary: boolean;
}

function isMonitor(v: unknown): v is MonitorInfo {
  if (typeof v !== "object" || v === null) return false;
  const m = v as Record<string, unknown>;
  return (
    typeof m.index === "number" &&
    typeof m.name === "string" &&
    typeof m.width === "number" &&
    typeof m.height === "number" &&
    typeof m.primary === "boolean"
  );
}

export type MouseButton = "left" | "right" | "middle" | "x1" | "x2";

/** Mensagens de entrada (viewer → host). Coordenadas normalizadas 0–65535. */
export type InputMessage =
  | { t: "mouse_move"; x: number; y: number }
  | { t: "mouse_button"; button: MouseButton; down: boolean; x: number; y: number }
  | { t: "wheel"; dx: number; dy: number }
  | { t: "key"; code: number; ext: boolean; down: boolean }
  | { t: "release_all" };

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
  if (
    o.t === "cursor_shape" &&
    typeof o.w === "number" &&
    typeof o.h === "number" &&
    typeof o.hx === "number" &&
    typeof o.hy === "number" &&
    typeof o.rgba === "string"
  ) {
    return { t: "cursor_shape", w: o.w, h: o.h, hx: o.hx, hy: o.hy, rgba: o.rgba };
  }
  if (
    o.t === "monitors" &&
    Array.isArray(o.list) &&
    o.list.every(isMonitor) &&
    typeof o.active === "number"
  ) {
    return { t: "monitors", list: o.list, active: o.active };
  }
  if (o.t === "cursor_visible" && typeof o.visible === "boolean") {
    return { t: "cursor_visible", visible: o.visible };
  }
  return null;
}
