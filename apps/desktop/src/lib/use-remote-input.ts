import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { useEffect, type RefObject } from "react";

import { WheelAccumulator, containBox, mouseButton, normalizePoint } from "@/lib/pointer";
import { scancodeFor } from "@/lib/scancodes";
import type { ViewerPeer } from "@/lib/viewer-peer";

interface HookKey {
  code: number;
  ext: boolean;
  down: boolean;
}

/**
 * Envia mouse e teclado do viewer ao host enquanto `active`.
 *
 * - Mouse: eventos do `<video>`, com coordenadas normalizadas sobre a área
 *   da imagem; movimentos agrupados por quadro de animação.
 * - Teclado: no app (Windows), o hook nativo captura tudo — inclusive Win e
 *   Alt+Tab — e chega pelo evento `viewer-key`. Sem o hook, usa `keydown`.
 * - Ao perder o foco ou sair, manda `release_all` (nada fica pressionado).
 */
export function useRemoteInput(
  video: RefObject<HTMLVideoElement | null>,
  peer: ViewerPeer | null,
  active: boolean,
  tauri: boolean,
) {
  useEffect(() => {
    const el = video.current;
    if (!el || !peer || !active) return;
    let disposed = false;
    let hookActive = false;
    let unlisten: (() => void) | null = null;
    let raf = 0;
    let pendingMove: { x: number; y: number } | null = null;
    const wheel = new WheelAccumulator();

    const point = (e: MouseEvent) => {
      const box = containBox(el.getBoundingClientRect(), el.videoWidth, el.videoHeight);
      return box ? normalizePoint(box, e.clientX, e.clientY) : null;
    };
    const flush = () => {
      raf = 0;
      if (pendingMove) peer.sendInput({ t: "mouse_move", ...pendingMove });
      pendingMove = null;
    };
    const onMove = (e: PointerEvent) => {
      const p = point(e);
      if (!p) return;
      pendingMove = p;
      if (!raf) raf = requestAnimationFrame(flush);
    };
    const onButton = (e: PointerEvent) => {
      const button = mouseButton(e.button);
      const p = point(e);
      if (!button || !p) return;
      e.preventDefault();
      const down = e.type === "pointerdown";
      if (down) el.setPointerCapture(e.pointerId);
      else if (el.hasPointerCapture(e.pointerId)) el.releasePointerCapture(e.pointerId);
      // O movimento pendente vai antes do clique.
      if (raf) cancelAnimationFrame(raf);
      pendingMove = null;
      raf = 0;
      peer.sendInput({ t: "mouse_button", button, down, ...p });
    };
    const onWheel = (e: WheelEvent) => {
      e.preventDefault();
      const { dx, dy } = wheel.push(e.deltaX, e.deltaY, e.deltaMode);
      if (dx || dy) peer.sendInput({ t: "wheel", dx, dy });
    };
    const prevent = (e: Event) => e.preventDefault();

    const sendKey = (code: number, ext: boolean, down: boolean) =>
      peer.sendInput({ t: "key", code, ext, down });
    const onKey = (e: KeyboardEvent) => {
      if (hookActive) return;
      const sc = scancodeFor(e.code);
      if (!sc) return;
      e.preventDefault();
      sendKey(sc.code, sc.ext, e.type === "keydown");
    };
    const releaseAll = () => peer.sendInput({ t: "release_all" });

    el.addEventListener("pointermove", onMove);
    el.addEventListener("pointerdown", onButton);
    el.addEventListener("pointerup", onButton);
    el.addEventListener("wheel", onWheel, { passive: false });
    el.addEventListener("contextmenu", prevent);
    el.addEventListener("auxclick", prevent);
    window.addEventListener("keydown", onKey);
    window.addEventListener("keyup", onKey);
    window.addEventListener("blur", releaseAll);

    if (tauri) {
      invoke<boolean>("set_keyboard_capture", { enabled: true })
        .then((v) => {
          if (!disposed) hookActive = v;
        })
        .catch(() => undefined);
      listen<HookKey>("viewer-key", (e) => sendKey(e.payload.code, e.payload.ext, e.payload.down))
        .then((u) => {
          if (disposed) u();
          else unlisten = u;
        })
        .catch(() => undefined);
    }

    return () => {
      disposed = true;
      if (raf) cancelAnimationFrame(raf);
      el.removeEventListener("pointermove", onMove);
      el.removeEventListener("pointerdown", onButton);
      el.removeEventListener("pointerup", onButton);
      el.removeEventListener("wheel", onWheel);
      el.removeEventListener("contextmenu", prevent);
      el.removeEventListener("auxclick", prevent);
      window.removeEventListener("keydown", onKey);
      window.removeEventListener("keyup", onKey);
      window.removeEventListener("blur", releaseAll);
      releaseAll();
      if (tauri) void invoke("set_keyboard_capture", { enabled: false }).catch(() => undefined);
      unlisten?.();
    };
  }, [video, peer, active, tauri]);
}
