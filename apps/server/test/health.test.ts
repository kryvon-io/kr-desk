import { afterAll, beforeAll, describe, expect, it } from "vitest";

import { MemoryBus } from "../src/bus.js";
import { buildApp, PROTOCOL_VERSION, SERVICE_VERSION } from "../src/build-app.js";
import { MemoryStore } from "../src/store.js";
import { StunOnly } from "../src/turn.js";

describe("GET /health", () => {
  let app: Awaited<ReturnType<typeof buildApp>>;
  beforeAll(async () => {
    app = await buildApp({ bus: new MemoryBus(), store: new MemoryStore(), turn: new StunOnly() });
  });
  afterAll(() => app.close());

  it("responde ok com a versão do protocolo", async () => {
    const res = await app.inject({ method: "GET", url: "/health" });
    expect(res.statusCode).toBe(200);
    expect(res.json()).toEqual({
      status: "ok",
      service: "kr-desk-signaling",
      version: SERVICE_VERSION,
      protocolVersion: PROTOCOL_VERSION,
    });
  });

  it("retorna 404 para rotas desconhecidas", async () => {
    const res = await app.inject({ method: "GET", url: "/nao-existe" });
    expect(res.statusCode).toBe(404);
  });
});
