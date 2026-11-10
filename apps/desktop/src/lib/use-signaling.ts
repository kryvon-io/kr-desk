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

  const closeViewer = useCallback((requestId?: string) => {
    if (viewer.current && (!requestId || viewer.current.requestId === requestId)) {
      viewer.current.peer.close();
      viewer.current = null;
      setRemote(null);
    }
  }, []);

  const startViewer = useCallback(
    (requestId: string, iceServers: IceServerDto[]) => {
      closeViewer();
      const update = (patch: Omit<Extract<Action, { type: "p2p_status" }>, "type" | "requestId">) =>
        dispatch({ type: "p2p_status", requestId, ...patch });
      const peer = new ViewerPeer(iceServers, {
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
      });
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

    const onMessage = (msg: ServerMessage) => {
      if (msg.type === "connect_accepted" && msg.request_id === outgoingId.current) {
        startViewer(msg.request_id, msg.ice_servers ?? []);
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
          }),
        ),
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

  const respond = useCallback(async (requestId: string, accept: boolean) => {
    dispatch({ type: "incoming_answered", requestId, accepted: accept });
    try {
      await invoke("respond_request", { requestId, accept });
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

  const resetOutgoing = useCallback(() => dispatch({ type: "outgoing_reset" }), []);
  const dismissNotice = useCallback(() => dispatch({ type: "notice_dismissed" }), []);

  return {
    state,
    remote,
    deviceName,
    tauri,
    connectTo,
    respond,
    endSession,
    resetOutgoing,
    dismissNotice,
  };
}
