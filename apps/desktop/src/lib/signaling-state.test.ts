import { describe, expect, it } from "vitest";

import {
  formatId,
  initialState,
  reducer,
  sanitizeIdInput,
  type Action,
  type SignalingState,
} from "./signaling-state";

const RID = "6f1c2b9e-3d4a-4c5b-9e8f-0a1b2c3d4e5f";
const online: Action = { type: "status", status: { state: "online", deviceId: "111222333" } };
const run = (...actions: Action[]): SignalingState => actions.reduce(reducer, initialState);
const msg = (m: Extract<Action, { type: "message" }>["msg"]): Action => ({
  type: "message",
  msg: m,
  now: 1000,
});

describe("lado de quem acessa (viewer)", () => {
  const pending = [
    online,
    { type: "connect_started", targetId: "444555666" },
    { type: "connect_sent", requestId: RID, targetId: "444555666" },
  ] satisfies Action[];

  it("pedido aceito vira sessão ativa", () => {
    const s = run(
      ...pending,
      msg({
        type: "connect_accepted",
        request_id: RID,
        peer_id: "444555666",
        peer_public_key: "k",
      }),
    );
    expect(s.outgoing).toEqual({ kind: "accepted", requestId: RID, peerId: "444555666" });
    expect(s.active).toMatchObject({ requestId: RID, peerId: "444555666", role: "viewer" });
    expect(s.active?.p2p.state).toBe("connecting");
  });

  it("recusa e timeout mostram mensagens diferentes", () => {
    const declined = run(
      ...pending,
      msg({ type: "connect_rejected", request_id: RID, reason: "declined" }),
    );
    const timeout = run(
      ...pending,
      msg({ type: "connect_rejected", request_id: RID, reason: "timeout" }),
    );
    expect(declined.outgoing).toMatchObject({
      kind: "failed",
      message: expect.stringContaining("recusou"),
    });
    expect(timeout.outgoing).toMatchObject({
      kind: "failed",
      message: expect.stringContaining("Sem resposta"),
    });
  });

  it("alvo offline, mesmo chegando antes da confirmação do envio", () => {
    const s = run(
      online,
      { type: "connect_started", targetId: "444555666" },
      msg({ type: "error", code: "target_offline", message: "x", request_id: RID }),
    );
    expect(s.outgoing).toMatchObject({
      kind: "failed",
      message: expect.stringContaining("offline"),
    });
  });

  it("ignora mensagens de outros pedidos", () => {
    const s = run(
      ...pending,
      msg({ type: "connect_rejected", request_id: "outro", reason: "declined" }),
    );
    expect(s.outgoing.kind).toBe("pending");
  });

  it("perder a conexão com o servidor falha o pedido pendente e mantém o último ID", () => {
    const s = run(...pending, { type: "status", status: { state: "connecting" } });
    expect(s.outgoing).toMatchObject({ kind: "failed" });
    expect(s.deviceId).toBe("111222333");
  });
});

describe("lado de quem é acessado (host)", () => {
  const incoming = msg({
    type: "incoming_request",
    request_id: RID,
    from_id: "777888999",
    from_name: "Notebook",
    from_public_key: "k",
  });

  it("enfileira o pedido sem duplicar", () => {
    const s = run(online, incoming, incoming);
    expect(s.incoming).toEqual([
      { requestId: RID, fromId: "777888999", fromName: "Notebook", receivedAt: 1000 },
    ]);
  });

  it("aceitar remove da fila e session_ready ativa a sessão", () => {
    const s = run(
      online,
      incoming,
      { type: "incoming_answered", requestId: RID, accepted: true },
      msg({ type: "session_ready", request_id: RID, peer_id: "777888999", peer_public_key: "k" }),
    );
    expect(s.incoming).toEqual([]);
    expect(s.active).toMatchObject({ requestId: RID, peerId: "777888999", role: "host" });
  });

  it("pedido expirado some e gera aviso", () => {
    const s = run(online, incoming, msg({ type: "session_ended", request_id: RID }));
    expect(s.incoming).toEqual([]);
    expect(s.notice).toContain("expirou");
  });

  it("o outro lado encerrar limpa a sessão com aviso", () => {
    const s = run(
      online,
      msg({ type: "session_ready", request_id: RID, peer_id: "777888999", peer_public_key: "k" }),
      msg({ type: "session_ended", request_id: RID }),
    );
    expect(s.active).toBeNull();
    expect(s.notice).toContain("encerrou");
  });
});

describe("estado P2P", () => {
  const ready = msg({
    type: "session_ready",
    request_id: RID,
    peer_id: "777888999",
    peer_public_key: "k",
  });
  const route = { local: "srflx", remote: "host", protocol: "udp", relayed: false } as const;

  it("atualiza estado, latência e rota só da sessão ativa", () => {
    const s = run(
      online,
      ready,
      { type: "p2p_status", requestId: RID, state: "connected" },
      { type: "p2p_status", requestId: RID, rttMs: 12 },
      { type: "p2p_status", requestId: RID, route },
      { type: "p2p_status", requestId: "outra", state: "failed" },
    );
    expect(s.active?.p2p).toEqual({ state: "connected", rttMs: 12, route, sharing: false });
  });

  it("marca quando a tela do host está sendo transmitida", () => {
    const s = run(online, ready, { type: "p2p_status", requestId: RID, sharing: true });
    expect(s.active?.p2p.sharing).toBe(true);
  });
});

describe("formatação de ID", () => {
  it("formata e limpa", () => {
    expect(formatId("123456789")).toBe("123 456 789");
    expect(formatId("1234")).toBe("123 4");
    expect(sanitizeIdInput("123 456-789 0")).toBe("123456789");
  });
});
