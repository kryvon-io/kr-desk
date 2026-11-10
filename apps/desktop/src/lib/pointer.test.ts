import { describe, expect, it } from "vitest";

import { WheelAccumulator, containBox, cursorCss, mouseButton, normalizePoint } from "./pointer";

describe("pointer", () => {
  it("considera as faixas pretas do object-fit: contain", () => {
    // Elemento 1000×1000 com vídeo 16:9 → faixas em cima e embaixo.
    const box = containBox({ left: 0, top: 0, width: 1000, height: 1000 }, 1920, 1080)!;
    expect(box.width).toBeCloseTo(1000);
    expect(box.height).toBeCloseTo(562.5);
    expect(box.top).toBeCloseTo(218.75);
    expect(normalizePoint(box, 0, box.top)).toEqual({ x: 0, y: 0 });
    expect(normalizePoint(box, 1000, box.top + box.height)).toEqual({ x: 65535, y: 65535 });
    expect(normalizePoint(box, 500, 500)).toEqual({ x: 32768, y: 32768 });
    // Fora da imagem fica preso na borda.
    expect(normalizePoint(box, -50, 0)).toEqual({ x: 0, y: 0 });
    expect(containBox({ left: 0, top: 0, width: 10, height: 10 }, 0, 0)).toBeNull();
  });

  it("botões", () => {
    expect(mouseButton(0)).toBe("left");
    expect(mouseButton(2)).toBe("right");
    expect(mouseButton(4)).toBe("x2");
    expect(mouseButton(7)).toBeNull();
  });

  it("roda: pixels, linhas e acúmulo de frações", () => {
    const w = new WheelAccumulator();
    expect(w.push(0, 100, 0)).toEqual({ dx: 0, dy: -120 });
    expect(w.push(0, -3, 1)).toEqual({ dx: 0, dy: 120 });
    expect(w.push(0, -0.5, 0)).toEqual({ dx: 0, dy: 0 });
    expect(w.push(0, -0.5, 0)).toEqual({ dx: 0, dy: 1 });
  });

  it("cursor CSS com hotspot e validação de tamanho", () => {
    const msg = { w: 1, h: 1, hx: 5, hy: 0, rgba: btoa("ÿ\u0000\u0000ÿ") };
    expect(cursorCss(msg, () => "data:x")).toBe("url(data:x) 0 0, default");
    expect(cursorCss({ ...msg, w: 2 }, () => "data:x")).toBeNull();
  });
});
