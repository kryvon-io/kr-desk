/**
 * Economia de recursos: presença e "última atividade" não podem gerar um
 * comando no Redis ou uma escrita no banco a cada ping/reconexão.
 */
import { afterEach, describe, expect, it } from "vitest";

import { MemoryBus } from "../src/bus.js";
import { depsFromEnv, trustProxyFromEnv } from "../src/config.js";
import { MemoryStore } from "../src/store.js";
import { connectDevice, newIdentity, startServer } from "./helpers.js";

/** Bus em memória que conta gravações. */
class CountingBus extends MemoryBus {
  sets = 0;
  override async set(key: string, value: string, ttlSec: number) {
    this.sets++;
    return super.set(key, value, ttlSec);
  }
}

const closers: Array<() => Promise<unknown>> = [];
afterEach(async () => {
  for (const c of closers.splice(0)) await c();
});

const log = { warn() {}, info() {}, error() {} };

describe("economia de recursos", () => {
  it("renova a presença no máximo uma vez por intervalo, não a cada ping", async () => {
    const bus = new CountingBus();
    const { app, url } = await startServer({
      bus,
      store: new MemoryStore(),
      turn: { iceServers: async () => [] },
      hubConfig: { presenceRefreshMs: 60_000 },
    });
    closers.push(() => app.close());
    const { client } = await connectDevice(url, newIdentity());
    closers.push(async () => client.close());
    const afterAuth = bus.sets;
    for (let i = 0; i < 5; i++) {
      client.send({ type: "ping", nonce: i });
      await client.next("pong");
    }
    expect(bus.sets).toBe(afterAuth);
  });

  it("só grava a última atividade no banco quando está velha ou algo mudou", async () => {
    const store = new MemoryStore();
    const { app, url } = await startServer({
      bus: new MemoryBus(),
      store,
      turn: { iceServers: async () => [] },
    });
    closers.push(() => app.close());
    const identity = newIdentity();
    for (let i = 0; i < 3; i++) {
      const { client } = await connectDevice(url, identity, "PC");
      client.close();
    }
    // Primeiro login cria; os dois seguintes não reescrevem nada.
    expect(store.touches).toBe(0);
    const { client } = await connectDevice(url, identity, "PC renomeado");
    client.close();
    expect(store.touches).toBe(1);
  });

  it("numa VPS de processo único dispensa o Redis em produção", async () => {
    const deps = await depsFromEnv(
      { NODE_ENV: "production", SIGNALING_SINGLE_INSTANCE: "1", DATABASE_URL: "" },
      log,
    ).catch((e: unknown) => e);
    // Sem banco, ainda falha (o banco continua obrigatório)…
    expect(String(deps)).toContain("DATABASE_URL");
    // …mas não por falta de Redis.
    expect(String(deps)).not.toContain("REDIS_URL");
    await expect(depsFromEnv({ NODE_ENV: "production" }, log)).rejects.toThrow("REDIS_URL");
  });

  it("TRUST_PROXY limita em quantos proxies confiar", () => {
    expect(trustProxyFromEnv({})).toBe(true);
    expect(trustProxyFromEnv({ TRUST_PROXY: "false" })).toBe(false);
    const two = trustProxyFromEnv({ TRUST_PROXY: "2" });
    expect(typeof two).toBe("function");
    const fn = two as (a: string, hop: number) => boolean;
    expect([fn("x", 0), fn("x", 1), fn("x", 2)]).toEqual([true, true, false]);
  });
});
