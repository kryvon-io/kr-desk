import { invoke, isTauri } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { useCallback, useEffect, useReducer, useRef, useState } from "react";

import {
  initialState,
  reducer,
  type Action,
  type ConnectionStatus,
  type ServerMessage,
} from "@/lib/signaling-state";
import type { MonitorInfo } from "@/lib/control";
import type { SessionCrypto } from "@/lib/session-auth";
import { cursorCss, rgbaToDataUrl } from "@/lib/pointer";
import {
  ViewerPeer,
  type IceServerDto,
  type PeerState,
  type RouteInfo,
  type SignalPayload,
} from "@/lib/viewer-peer";

interface SignalingStatusDto {
  status: ConnectionStatus;
  deviceName: string;
}

/** Evento `p2p-status` emitido pelo Rust quando este computador é o host. */
interface HostP2pStatus {
  requestId: string;
  state: PeerState;
  rttMs: number | null;
  route: RouteInfo | null;
  sharing: boolean;
  control: boolean;
  verificationCode: string | null;
  authenticated: boolean;
  firstContact: boolean;
  securityError: string | null;
  peerId: string;
}

/** Assinatura e verificação pelo Rust (a chave do dispositivo fica lá). */
function tauriSessionCrypto(peerId: string, expectedPublicKey: string): SessionCrypto {
  return {
    sign: (message) =>
      invoke<{ publicKey: string; signature: string }>("sign_session", { message }),
    verify: async (publicKey, signature, message) => {
      const r = await invoke<
        | { result: "ok"; firstContact: boolean }
        | { result: "changed" }
        | { result: "failed"; reason: string }
      >("verify_session", { peerId, expectedPublicKey, publicKey, signature, message });
      return r;
    },
  };
}

/** Converte o erro do `invoke` (string vinda do Rust) em texto. */
function errorMessage(err: unknown): string {
  const text = typeof err === "string" ? err : err instanceof Error ? err.message : String(err);
  if (/sem conexão/i.test(text)) return "Sem conexão com o servidor. Tentando reconectar…";
  return text;
}

/** Liga a interface ao núcleo Rust: estado, eventos, ações e a conexão P2P do viewer. */
export function useSignaling() {
  const [state, dispatch] = useReducer(reducer, initialState);
  const [deviceName, setDeviceName] = useState<string>("");
  const tauri = isTauri();

  /** Pedido feito por este computador (para casar `connect_accepted` e sinais). */
  const outgoingId = useRef<string | null>(null);
  /** Conexão WebRTC quando este computador é o viewer. */
  const viewer = useRef<{ requestId: string; peer: ViewerPeer } | null>(null);
  /** Vídeo da tela remota (viewer) e a conexão que o recebe. */
  const [remote, setRemote] = useState<{ peer: ViewerPeer; stream: MediaStream } | null>(null);
  /** Cursor do host como valor CSS (desenhado localmente, sem atraso). */
  const [remoteCursor, setRemoteCursor] = useState("default");
  /** Monitores do host (viewer). */
  const [remoteMonitors, setRemoteMonitors] = useState<{
    list: MonitorInfo[];
    active: number;
  } | null>(null);
  const cursorShape = useRef("default");
  const cursorVisible = useRef(true);

  const closeViewer = useCallback((requestId?: string) => {
    if (viewer.current && (!requestId || viewer.current.requestId === requestId)) {
      viewer.current.peer.close();
      viewer.current = null;
      setRemote(null);
      cursorShape.current = "default";
      cursorVisible.current = true;
      setRemoteCursor("default");
      setRemoteMonitors(null);
    }
  }, []);

  const startViewer = useCallback(
    (requestId: string, iceServers: IceServerDto[], peerId: string, peerPublicKey: string) => {
      closeViewer();
      const update = (patch: Omit<Extract<Action, { type: "p2p_status" }>, "type" | "requestId">) =>
        dispatch({ type: "p2p_status", requestId, ...patch });
      const peer = new ViewerPeer(
        iceServers,
        {
          onSignal: (payload) => {
            invoke("send_signal", { requestId, payload }).catch((err: unknown) =>
              console.warn("falha ao enviar sinal", err),
            );
          },
          onState: (s) => update({ state: s }),
          onRtt: (ms) => update({ rttMs: ms }),
          onRoute: (route) => update({ route }),
          onVideo: (stream) => {
            if (viewer.current?.peer === peer) setRemote({ peer, stream });
          },
          onVerificationCode: (code) => update({ verificationCode: code }),
          onPermissions: (control) => update({ control }),
          onAuth: (outcome) => {
            if (outcome.result === "ok") {
              update({ authenticated: true, firstContact: outcome.firstContact });
              return;
            }
            // Não confiamos no outro lado: encerra a sessão antes de qualquer entrada.
            dispatch({
              type: "security_failed",
              alert: {
                peerId,
                reason:
                  outcome.result === "changed"
                    ? "A identidade deste computador mudou desde a última conexão."
                    : `Não foi possível confirmar a identidade do outro computador (${outcome.reason}).`,
                keyChanged: outcome.result === "changed",
              },
            });
            closeViewer(requestId);
            dispatch({ type: "session_closed", requestId });
            invoke("end_session", { requestId }).catch(() => undefined);
          },
          onMonitors: (list, active) => {
            if (viewer.current?.peer === peer) setRemoteMonitors({ list, active });
          },
          onCursor: (msg) => {
            if (msg.t === "cursor_shape") {
              cursorShape.current = cursorCss(msg, rgbaToDataUrl) ?? "default";
            } else {
              cursorVisible.current = msg.visible;
            }
            setRemoteCursor(cursorVisible.current ? cursorShape.current : "none");
          },
        },
        { auth: { requestId, crypto: tauriSessionCrypto(peerId, peerPublicKey) } },
      );
      viewer.current = { requestId, peer };
      peer.start().catch((err: unknown) => {
        console.warn("falha ao iniciar a conexão P2P", err);
        update({ state: "failed" });
      });
    },
    [closeViewer],
  );

  useEffect(() => {
    if (!tauri) return;
    let disposed = false;
    const unlisten: Array<() => void> = [];
    const reportedSecurity = new Set<string>();

    const onMessage = (msg: ServerMessage) => {
      if (msg.type === "connect_accepted" && msg.request_id === outgoingId.current) {
        startViewer(msg.request_id, msg.ice_servers ?? [], msg.peer_id, msg.peer_public_key);
      } else if (msg.type === "signal" && msg.request_id === viewer.current?.requestId) {
        viewer.current.peer
          .handleSignal(msg.payload as SignalPayload)
          .catch((err: unknown) => console.warn("sinal P2P inválido", err));
      } else if (msg.type === "session_ended") {
        closeViewer(msg.request_id);
      }
      dispatch({ type: "message", msg, now: Date.now() });
    };

    void (async () => {
      unlisten.push(
        await listen<ConnectionStatus>("signaling-status", (e) =>
          dispatch({ type: "status", status: e.payload }),
        ),
        await listen<ServerMessage>("signaling-message", (e) => onMessage(e.payload)),
        await listen<HostP2pStatus>("p2p-status", (e) =>
          dispatch({
            type: "p2p_status",
            requestId: e.payload.requestId,
            state: e.payload.state,
            rttMs: e.payload.rttMs,
            route: e.payload.route,
            sharing: e.payload.sharing,
            control: e.payload.control,
            verificationCode: e.payload.verificationCode,
            authenticated: e.payload.authenticated,
            firstContact: e.payload.firstContact,
          }),
        ),
        await listen<HostP2pStatus>("p2p-status", (e) => {
          if (e.payload.securityError && !reportedSecurity.has(e.payload.requestId)) {
            reportedSecurity.add(e.payload.requestId);
            dispatch({
              type: "security_failed",
              alert: {
                peerId: e.payload.peerId,
                reason: `Sessão encerrada por segurança: ${e.payload.securityError}.`,
                keyChanged: e.payload.securityError.includes("mudou"),
              },
            });
          }
        }),
      );
      // Busca o estado atual depois de assinar, para não perder transições.
      const current = await invoke<SignalingStatusDto>("signaling_status");
      if (disposed) return;
      setDeviceName(current.deviceName);
      dispatch({ type: "status", status: current.status });
    })();

    return () => {
      disposed = true;
      unlisten.forEach((fn) => fn());
      closeViewer();
    };
  }, [tauri, startViewer, closeViewer]);

  const connectTo = useCallback(async (targetId: string) => {
    dispatch({ type: "connect_started", targetId });
    try {
      const requestId = await invoke<string>("connect_to", { targetId });
      outgoingId.current = requestId;
      dispatch({ type: "connect_sent", requestId, targetId });
    } catch (err) {
      dispatch({ type: "connect_failed", message: errorMessage(err) });
    }
  }, []);

  const respond = useCallback(async (requestId: string, accept: boolean, allowControl = true) => {
    dispatch({ type: "incoming_answered", requestId, accepted: accept });
    try {
      await invoke("respond_request", { requestId, accept, allowControl });
    } catch (err) {
      console.warn("falha ao responder pedido", err);
    }
  }, []);

  const endSession = useCallback(
    async (requestId: string) => {
      closeViewer(requestId);
      dispatch({ type: "session_closed", requestId });
      try {
        await invoke("end_session", { requestId });
      } catch (err) {
        console.warn("falha ao encerrar sessão", err);
      }
    },
    [closeViewer],
  );

  /** Host: liga/desliga o controle remoto durante a sessão. */
  const setSessionControl = useCallback((requestId: string, allow: boolean) => {
    invoke("set_session_control", { requestId, allow }).catch((err: unknown) =>
      console.warn("falha ao alterar permissão", err),
    );
  }, []);

  /** Esquece a chave antiga do computador (ele foi reinstalado). */
  const trustNewIdentity = useCallback(async (peerId: string) => {
    try {
      await invoke("forget_peer", { peerId });
    } finally {
      dispatch({ type: "security_dismissed" });
    }
  }, []);
  const dismissSecurity = useCallback(() => dispatch({ type: "security_dismissed" }), []);

  const resetOutgoing = useCallback(() => dispatch({ type: "outgoing_reset" }), []);
  const dismissNotice = useCallback(() => dispatch({ type: "notice_dismissed" }), []);

  return {
    state,
    remote,
    remoteCursor,
    remoteMonitors,
    deviceName,
    tauri,
    connectTo,
    respond,
    endSession,
    setSessionControl,
    trustNewIdentity,
    dismissSecurity,
    resetOutgoing,
    dismissNotice,
  };
}
