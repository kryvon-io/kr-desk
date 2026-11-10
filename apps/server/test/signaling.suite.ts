/**
 * Suíte de sinalização reutilizada com bus em memória e com Redis real.
 * Sempre sobe DUAS instâncias do servidor compartilhando o mesmo bus/banco,
 * com o viewer numa e o host na outra (como pode acontecer na Vercel).
 */
import { afterEach, beforeEach, describe, expect, it } from "vitest";

import type { Bus } from "../src/bus.js";
import { CLOSE_AUTH_TIMEOUT, CLOSE_ROTATE, type HubConfig } from "../src/hub.js";
import type { DeviceStore } from "../src/store.js";
import type { IceServer } from "../src/protocol.js";
import {
  connectDevice,
  newIdentity,
  newRequestId,
  signNonce,
  startServer,
  TestClient,
  type Msg,
} from "./helpers.js";

export interface SuiteEnv {
  /** Cria um bus por instância, todos enxergando o mesmo estado. */
  makeBus(): Bus;
  makeStore(): DeviceStore;
  cleanup?(): Promise<void>;
}

const ICE: IceServer[] = [
  { urls: ["stun:stun.cloudflare.com:3478"] },
  { urls: ["turn:turn.example:3478"], username: "u", credential: "c" },
];

export function signalingSuite(name: string, env: () => SuiteEnv) {
  describe(name, () => {
    let servers: Array<Awaited<ReturnType<typeof startServer>>> = [];
    let clients: TestClient[] = [];
    let suite: SuiteEnv;
    let store: DeviceStore;

    async function start(n = 2, hubConfig: Partial<HubConfig> = {}) {
      for (let i = 0; i < n; i++) {
        servers.push(
          await startServer({
            bus: suite.makeBus(),
            store,
            turn: { iceServers: async () => ICE },
            hubConfig: { pendingTimeoutMs: 5_000, ...hubConfig },
          }),
        );
      }
      return servers.map((s) => s.url);
    }

    async function device(url: string, identity = newIdentity(), label = "PC") {
      const d = await connectDevice(url, identity, label);
      clients.push(d.client);
      return d;
    }

    beforeEach(() => {
      suite = env();
      store = suite.makeStore();
    });

    afterEach(async () => {
      for (const c of clients) c.close();
      // O store é compartilhado: fecha só o bus de cada instância.
      await Promise.all(servers.map(({ app }) => (app.hub.deps.bus.close(), app.server.close())));
      await Promise.all(servers.map(({ app }) => app.hub.closeAll()));
      await store.close();
      await suite.cleanup?.();
      servers = [];
      clients = [];
    });

    it("registra com ID de 9 dígitos e mantém o mesmo ID no próximo login", async () => {
      const [a, b] = await start();
      const identity = newIdentity();
      const first = await device(a!, identity);
      expect(first.deviceId).toMatch(/^[1-9]\d{8}$/);
      first.client.close();
      const second = await device(b!, identity);
      expect(second.deviceId).toBe(first.deviceId);
    });

    it("recusa assinatura inválida", async () => {
      const [a] = await start(1);
      const c = new TestClient(a!);
      clients.push(c);
      await c.next("hello");
      const id = newIdentity();
      c.send({
        type: "auth",
        public_key: id.publicKey,
        signature: signNonce(id.privateKey, "outro-nonce"),
        device_name: "x",
        app_version: "0.1.0",
      });
      expect(await c.next("error")).toMatchObject({ code: "auth_failed" });
    });

    it("exige autenticação antes de outras mensagens", async () => {
      const [a] = await start(1);
      const c = new TestClient(a!);
      clients.push(c);
      await c.next("hello");
      c.send({ type: "connect_request", request_id: newRequestId(), target_id: "123456789" });
      expect(await c.next("error")).toMatchObject({ code: "auth_required" });
    });

    it("fluxo completo entre instâncias: pedido, aceite, sinais e encerramento", async () => {
      const [a, b] = await start();
      const viewer = await device(a!, newIdentity(), "Notebook");
      const host = await device(b!, newIdentity(), "Escritório");
      const rid = newRequestId();

      viewer.client.send({ type: "connect_request", request_id: rid, target_id: host.deviceId });
      expect(await viewer.client.next("connect_pending")).toMatchObject({ request_id: rid });

      const incoming = await host.client.next("incoming_request");
      expect(incoming).toMatchObject({
        request_id: rid,
        from_id: viewer.deviceId,
        from_name: "Notebook",
        from_public_key: viewer.identity.publicKey,
      });

      host.client.send({ type: "connect_response", request_id: rid, accept: true });
      expect(await viewer.client.next("connect_accepted")).toMatchObject({
        request_id: rid,
        peer_id: host.deviceId,
        peer_public_key: host.identity.publicKey,
        ice_servers: ICE,
      });
      expect(await host.client.next("session_ready")).toMatchObject({
        peer_id: viewer.deviceId,
        peer_public_key: viewer.identity.publicKey,
      });

      viewer.client.send({
        type: "signal",
        request_id: rid,
        payload: { kind: "offer", sdp: "v=0" },
      });
      expect(await host.client.next("signal")).toEqual({
        v: 1,
        type: "signal",
        request_id: rid,
        from_id: viewer.deviceId,
        payload: { kind: "offer", sdp: "v=0" },
      });
      host.client.send({
        type: "signal",
        request_id: rid,
        payload: { kind: "answer", sdp: "v=0" },
      });
      expect(await viewer.client.next("signal")).toMatchObject({ from_id: host.deviceId });

      host.client.send({ type: "end_session", request_id: rid });
      expect(await viewer.client.next("session_ended")).toMatchObject({ request_id: rid });

      viewer.client.send({ type: "signal", request_id: rid, payload: { kind: "x" } });
      expect(await viewer.client.next("error")).toMatchObject({ code: "unknown_request" });
    });

    it("host recusa", async () => {
      const [a, b] = await start();
      const viewer = await device(a!);
      const host = await device(b!);
      const rid = newRequestId();
      viewer.client.send({ type: "connect_request", request_id: rid, target_id: host.deviceId });
      await host.client.next("incoming_request");
      host.client.send({ type: "connect_response", request_id: rid, accept: false });
      expect(await viewer.client.next("connect_rejected")).toMatchObject({ reason: "declined" });
    });

    it("sem resposta do host → recusa por timeout e o host é avisado", async () => {
      const [a, b] = await start(2, { pendingTimeoutMs: 300 });
      const viewer = await device(a!);
      const host = await device(b!);
      const rid = newRequestId();
      viewer.client.send({ type: "connect_request", request_id: rid, target_id: host.deviceId });
      await host.client.next("incoming_request");
      expect(await viewer.client.next("connect_rejected")).toMatchObject({ reason: "timeout" });
      expect(await host.client.next("session_ended")).toMatchObject({ request_id: rid });
      host.client.send({ type: "connect_response", request_id: rid, accept: true });
      expect(await host.client.next("error")).toMatchObject({ code: "unknown_request" });
    });

    it("alvo offline, conexão consigo mesmo e terceiro sem permissão", async () => {
      const [a, b] = await start();
      const viewer = await device(a!);
      const host = await device(b!);
      const intruder = await device(a!);

      viewer.client.send({
        type: "connect_request",
        request_id: newRequestId(),
        target_id: "999999999",
      });
      expect(await viewer.client.next("error")).toMatchObject({ code: "target_offline" });

      viewer.client.send({
        type: "connect_request",
        request_id: newRequestId(),
        target_id: viewer.deviceId,
      });
      expect(await viewer.client.next("error")).toMatchObject({ code: "self_connect" });

      const rid = newRequestId();
      viewer.client.send({ type: "connect_request", request_id: rid, target_id: host.deviceId });
      await host.client.next("incoming_request");
      // Um terceiro não pode aceitar nem injetar sinais na sessão alheia.
      intruder.client.send({ type: "connect_response", request_id: rid, accept: true });
      expect(await intruder.client.next("error")).toMatchObject({ code: "unknown_request" });
      host.client.send({ type: "connect_response", request_id: rid, accept: true });
      await viewer.client.next("connect_accepted");
      intruder.client.send({ type: "signal", request_id: rid, payload: { kind: "offer" } });
      expect(await intruder.client.next("error")).toMatchObject({ code: "unknown_request" });
      await host.client.expectNone("signal");
    });

    it("reconexão com sobreposição: só a conexão nova recebe e a presença continua", async () => {
      const [a, b] = await start();
      const viewer = await device(a!);
      const hostId = newIdentity();
      const oldHost = await device(b!, hostId);
      const newHost = await device(a!, hostId);
      expect(newHost.deviceId).toBe(oldHost.deviceId);

      // Fechar a conexão antiga NÃO pode tirar o dispositivo do ar.
      oldHost.client.close();
      await oldHost.client.closed;
      await new Promise((r) => setTimeout(r, 50));

      const rid = newRequestId();
      viewer.client.send({ type: "connect_request", request_id: rid, target_id: newHost.deviceId });
      expect(await newHost.client.next("incoming_request")).toMatchObject({ request_id: rid });
    });

    it("rotação: avisa reconnect_soon e fecha com código 4000", async () => {
      const [a] = await start(1, { maxLifetimeMs: 400, reconnectNoticeMs: 200 });
      const d = await device(a!);
      expect(await d.client.next<Msg & { in_ms: number }>("reconnect_soon")).toMatchObject({
        in_ms: 200,
      });
      expect((await d.client.closed).code).toBe(CLOSE_ROTATE);
    });

    it("fecha conexões que não se autenticam a tempo", async () => {
      const [a] = await start(1, { authTimeoutMs: 150 });
      const c = new TestClient(a!);
      clients.push(c);
      expect((await c.closed).code).toBe(CLOSE_AUTH_TIMEOUT);
    });

    it("ping renova presença e responde pong", async () => {
      const [a] = await start(1);
      const d = await device(a!);
      d.client.send({ type: "ping", nonce: 9 });
      expect(await d.client.next("pong")).toMatchObject({ nonce: 9 });
    });
  });
}
