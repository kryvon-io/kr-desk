import { afterAll, describe, expect, it } from "vitest";

import { buildApp, PROTOCOL_VERSION } from "../src/build-app.js";

describe("GET /health", () => {
  const app = buildApp();
  afterAll(() => app.close());

  it("responde ok com a versão do protocolo", async () => {
    const res = await app.inject({ method: "GET", url: "/health" });
    expect(res.statusCode).toBe(200);
    expect(res.json()).toEqual({
      status: "ok",
      service: "kr-desk-signaling",
      version: "0.1.0",
      protocolVersion: PROTOCOL_VERSION,
    });
  });

  it("retorna 404 para rotas desconhecidas", async () => {
    const res = await app.inject({ method: "GET", url: "/nao-existe" });
    expect(res.statusCode).toBe(404);
  });
});
