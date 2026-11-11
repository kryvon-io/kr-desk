import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { useEffect } from "react";

import type { ViewerPeer } from "@/lib/viewer-peer";

/**
 * Viewer: enquanto `enabled`, o texto copiado neste computador (observado
 * pelo Rust) vai para o host. O sentido contrário é tratado em
 * `use-signaling` (evento `onClipboard` do `ViewerPeer`).
 */
export function useClipboardSync(peer: ViewerPeer | null, enabled: boolean, tauri: boolean) {
  useEffect(() => {
    if (!tauri || !peer || !enabled) return;
    let disposed = false;
    let unlisten: (() => void) | null = null;
    invoke("set_viewer_clipboard", { enabled: true }).catch(() => undefined);
    listen<string>("local-clipboard", (e) => peer.sendClipboard(e.payload))
      .then((u) => {
        if (disposed) u();
        else unlisten = u;
      })
      .catch(() => undefined);
    return () => {
      disposed = true;
      unlisten?.();
      invoke("set_viewer_clipboard", { enabled: false }).catch(() => undefined);
    };
  }, [peer, enabled, tauri]);
}
