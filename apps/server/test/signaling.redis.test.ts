/**
 * Mesma suíte, com Redis e Postgres reais. Roda quando TEST_REDIS_URL e
 * TEST_DATABASE_URL estão definidos (no CI, via serviços do GitHub Actions).
 */
import { randomUUID } from "node:crypto";

import { Redis } from "ioredis";
import { describe, it } from "vitest";

import { RedisBus } from "../src/bus.js";
import { createDb } from "../src/db/client.js";
import { PrismaStore } from "../src/store.js";
import { signalingSuite } from "./signaling.suite.js";

const redisUrl = process.env["TEST_REDIS_URL"];
const databaseUrl = process.env["TEST_DATABASE_URL"];

if (redisUrl && databaseUrl) {
  signalingSuite("sinalização (Redis + Postgres reais)", () => {
    // Prefixo por teste evitaria colisões, mas os IDs/UUIDs já são aleatórios.
    return {
      makeBus: () => new RedisBus(redisUrl),
      makeStore: () => new PrismaStore(createDb(databaseUrl)),
      cleanup: async () => {
        const r = new Redis(redisUrl);
        await r.flushdb();
        await r.quit();
      },
    };
  });
} else {
  describe.skip("sinalização (Redis + Postgres reais)", () => {
    it(`defina TEST_REDIS_URL e TEST_DATABASE_URL (${randomUUID().slice(0, 0)})`, () => {});
  });
}
