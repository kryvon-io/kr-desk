// Página de teste: o mesmo `ViewerPeer` do app, rodando num Chrome comum.
import { ViewerPeer, type SignalPayload } from "../../apps/desktop/src/lib/viewer-peer";

interface InteropResult {
  ok: boolean;
  state?: string;
  rttMs?: number;
  route?: string;
  error?: string;
}

declare global {
  interface Window {
    __interop: InteropResult;
  }
}

window.__interop = { ok: false };
const ws = new WebSocket(new URLSearchParams(location.search).get("ws") ?? "");

ws.onopen = () => {
  const peer = new ViewerPeer([], {
    onSignal: (p) => ws.send(JSON.stringify(p)),
    onState: (s) => (window.__interop.state = s),
    onRtt: (ms) => {
      window.__interop.rttMs = ms;
      window.__interop.ok = window.__interop.route !== undefined;
    },
    onRoute: (r) => (window.__interop.route = `${r.local}/${r.remote}/${r.protocol}`),
  });
  ws.onmessage = (e) => {
    peer.handleSignal(JSON.parse(String(e.data)) as SignalPayload).catch((err: unknown) => {
      window.__interop.error = String(err);
    });
  };
  peer.start().catch((err: unknown) => (window.__interop.error = String(err)));
};
ws.onerror = () => (window.__interop.error = "falha no WebSocket");
