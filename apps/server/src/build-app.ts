import websocket from "@fastify/websocket";
import Fastify, { type FastifyServerOptions } from "fastify";

import type { Bus } from "./bus.js";
import { SignalingHub, type HubConfig } from "./hub.js";
import { PROTOCOL_VERSION } from "./protocol.js";
import type { DeviceStore } from "./store.js";
import type { TurnProvider } from "./turn.js";

export { PROTOCOL_VERSION };
export const SERVICE_VERSION = "0.2.0";

/** Tamanho máximo de uma mensagem WebSocket (SDP cabe com folga). */
export const MAX_MESSAGE_BYTES = 64 * 1024;

export interface AppDeps {
  bus: Bus;
  store: DeviceStore;
  turn: TurnProvider;
  hubConfig?: Partial<HubConfig>;
}

/**
 * Monta a aplicação sem iniciar a escuta, para ser usada tanto pelo
 * entrypoint (`src/index.ts`, detectado pela Vercel) quanto pelos testes.
 */
export async function buildApp(deps: AppDeps, options: FastifyServerOptions = {}) {
  // trustProxy: atrás da Vercel o IP real vem em X-Forwarded-For (usado no rate limit).
  const app = Fastify({ trustProxy: true, ...options });
  const hub = new SignalingHub({
    bus: deps.bus,
    store: deps.store,
    turn: deps.turn,
    log: app.log,
    ...(deps.hubConfig ? { config: deps.hubConfig } : {}),
  });

  await app.register(websocket, { options: { maxPayload: MAX_MESSAGE_BYTES } });

  app.get("/health", async () => ({
    status: "ok" as const,
    service: "kr-desk-signaling",
    version: SERVICE_VERSION,
    protocolVersion: PROTOCOL_VERSION,
  }));

  app.get("/ws", { websocket: true }, (socket, request) => {
    hub.attach(socket, request.ip);
  });

  app.addHook("onClose", async () => {
    await hub.closeAll();
    await Promise.allSettled([deps.bus.close(), deps.store.close()]);
  });

  app.decorate("hub", hub);
  return app;
}

declare module "fastify" {
  interface FastifyInstance {
    hub: SignalingHub;
  }
}
