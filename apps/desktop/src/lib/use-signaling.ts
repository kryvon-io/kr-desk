import { invoke, isTauri } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { useCallback, useEffect, useReducer, useState } from "react";

import {
  initialState,
  reducer,
  type ConnectionStatus,
  type ServerMessage,
} from "@/lib/signaling-state";

interface SignalingStatusDto {
  status: ConnectionStatus;
  deviceName: string;
}

/** Converte o erro do `invoke` (string vinda do Rust) em texto. */
function errorMessage(err: unknown): string {
  const text = typeof err === "string" ? err : err instanceof Error ? err.message : String(err);
  if (/sem conexão/i.test(text)) return "Sem conexão com o servidor. Tentando reconectar…";
  return text;
}

/** Liga a interface ao núcleo Rust: estado, eventos e ações. */
export function useSignaling() {
  const [state, dispatch] = useReducer(reducer, initialState);
  const [deviceName, setDeviceName] = useState<string>("");
  const tauri = isTauri();

  useEffect(() => {
    if (!tauri) return;
    let disposed = false;
    const unlisten: Array<() => void> = [];

    void (async () => {
      unlisten.push(
        await listen<ConnectionStatus>("signaling-status", (e) =>
          dispatch({ type: "status", status: e.payload }),
        ),
        await listen<ServerMessage>("signaling-message", (e) =>
          dispatch({ type: "message", msg: e.payload, now: Date.now() }),
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
    };
  }, [tauri]);

  const connectTo = useCallback(async (targetId: string) => {
    dispatch({ type: "connect_started", targetId });
    try {
      const requestId = await invoke<string>("connect_to", { targetId });
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

  const endSession = useCallback(async (requestId: string) => {
    dispatch({ type: "session_closed", requestId });
    try {
      await invoke("end_session", { requestId });
    } catch (err) {
      console.warn("falha ao encerrar sessão", err);
    }
  }, []);

  const resetOutgoing = useCallback(() => dispatch({ type: "outgoing_reset" }), []);
  const dismissNotice = useCallback(() => dispatch({ type: "notice_dismissed" }), []);

  return {
    state,
    deviceName,
    tauri,
    connectTo,
    respond,
    endSession,
    resetOutgoing,
    dismissNotice,
  };
}
