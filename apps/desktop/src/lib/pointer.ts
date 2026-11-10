/**
 * Mapeamento do ponteiro e da roda do viewer para o protocolo de entrada.
 */
import type { MouseButton } from "./control";

/** Área ocupada pelo vídeo dentro do elemento (`object-fit: contain`). */
export interface ContentBox {
  left: number;
  top: number;
  width: number;
  height: number;
}

export function containBox(
  element: { left: number; top: number; width: number; height: number },
  videoWidth: number,
  videoHeight: number,
): ContentBox | null {
  if (videoWidth <= 0 || videoHeight <= 0 || element.width <= 0 || element.height <= 0) {
    return null;
  }
  const scale = Math.min(element.width / videoWidth, element.height / videoHeight);
  const width = videoWidth * scale;
  const height = videoHeight * scale;
  return {
    left: element.left + (element.width - width) / 2,
    top: element.top + (element.height - height) / 2,
    width,
    height,
  };
}

/** Posição do cliente → coordenada normalizada 0–65535 (limitada à imagem). */
export function normalizePoint(
  box: ContentBox,
  clientX: number,
  clientY: number,
): { x: number; y: number } {
  const fx = Math.min(1, Math.max(0, (clientX - box.left) / box.width));
  const fy = Math.min(1, Math.max(0, (clientY - box.top) / box.height));
  return { x: Math.round(fx * 65535), y: Math.round(fy * 65535) };
}

const BUTTONS: Record<number, MouseButton> = {
  0: "left",
  1: "middle",
  2: "right",
  3: "x1",
  4: "x2",
};

export function mouseButton(button: number): MouseButton | null {
  return BUTTONS[button] ?? null;
}

/** Pixels de rolagem do navegador por "clique" da roda (Chromium no Windows). */
const PIXELS_PER_NOTCH = 100;
const LINES_PER_NOTCH = 3;

/**
 * Converte `WheelEvent` para unidades do Windows (120 = um clique;
 * positivo = para cima/direita), acumulando frações entre eventos.
 */
export class WheelAccumulator {
  private x = 0;
  private y = 0;

  push(deltaX: number, deltaY: number, deltaMode: number): { dx: number; dy: number } {
    const perUnit =
      deltaMode === 1 ? 120 / LINES_PER_NOTCH : deltaMode === 2 ? 120 * 3 : 120 / PIXELS_PER_NOTCH;
    this.x += deltaX * perUnit;
    this.y += -deltaY * perUnit;
    const dx = Math.trunc(this.x);
    const dy = Math.trunc(this.y);
    this.x -= dx;
    this.y -= dy;
    return { dx, dy };
  }
}

/** Forma do cursor remoto (RGBA em base64) → valor CSS `cursor`. */
export function cursorCss(
  msg: { w: number; h: number; hx: number; hy: number; rgba: string },
  toDataUrl: (rgba: Uint8ClampedArray, w: number, h: number) => string,
): string | null {
  if (msg.w <= 0 || msg.h <= 0 || msg.w > 256 || msg.h > 256) return null;
  const bytes = Uint8ClampedArray.from(atob(msg.rgba), (c) => c.charCodeAt(0));
  if (bytes.length !== msg.w * msg.h * 4) return null;
  const hx = Math.min(msg.hx, msg.w - 1);
  const hy = Math.min(msg.hy, msg.h - 1);
  return `url(${toDataUrl(bytes, msg.w, msg.h)}) ${hx} ${hy}, default`;
}

/** Renderiza RGBA num canvas e devolve um PNG em data URL. */
export function rgbaToDataUrl(rgba: Uint8ClampedArray, w: number, h: number): string {
  const canvas = document.createElement("canvas");
  canvas.width = w;
  canvas.height = h;
  const ctx = canvas.getContext("2d");
  if (!ctx) return "";
  ctx.putImageData(new ImageData(new Uint8ClampedArray(rgba), w, h), 0, 0);
  return canvas.toDataURL("image/png");
}
