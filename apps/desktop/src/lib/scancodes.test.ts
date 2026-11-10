import { describe, expect, it } from "vitest";

import { scancodeFor } from "./scancodes";

describe("scancodeFor", () => {
  it("letras, números e modificadores", () => {
    expect(scancodeFor("KeyA")).toEqual({ code: 0x1e, ext: false });
    expect(scancodeFor("Digit0")).toEqual({ code: 0x0b, ext: false });
    expect(scancodeFor("ControlRight")).toEqual({ code: 0x1d, ext: true });
    expect(scancodeFor("MetaLeft")).toEqual({ code: 0x5b, ext: true });
  });

  it("teclas próprias do ABNT2", () => {
    expect(scancodeFor("IntlRo")).toEqual({ code: 0x73, ext: false });
    expect(scancodeFor("IntlBackslash")).toEqual({ code: 0x56, ext: false });
    expect(scancodeFor("NumpadComma")).toEqual({ code: 0x7e, ext: false });
  });

  it("setas são estendidas e diferem do teclado numérico", () => {
    expect(scancodeFor("ArrowLeft")).toEqual({ code: 0x4b, ext: true });
    expect(scancodeFor("Numpad4")).toEqual({ code: 0x4b, ext: false });
  });

  it("desconhecida", () => {
    expect(scancodeFor("Fn")).toBeNull();
  });
});
