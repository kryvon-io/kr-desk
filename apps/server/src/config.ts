import { MemoryBus, RedisBus, type Bus } from "./bus.js";
import type { AppDeps } from "./build-app.js";
import { MemoryStore, PrismaStore, type DeviceStore } from "./store.js";
import { CloudflareTurn, StunOnly, type Logger, type TurnProvider } from "./turn.js";

/**
 * Monta as dependências a partir das variáveis de ambiente.
 * Sem banco/Redis configurados (dev local), cai para implementações em memória.
 */
export async function depsFromEnv(env: NodeJS.ProcessEnv, log: Logger): Promise<AppDeps> {
  const isProd = env["VERCEL_ENV"] === "production" || env["NODE_ENV"] === "production";

  // A integração Upstash da Vercel pode expor a URL com nomes diferentes.
  const redisUrl = env["REDIS_URL"] ?? env["KV_URL"] ?? env["UPSTASH_REDIS_URL"];
  let bus: Bus;
  if (redisUrl) bus = new RedisBus(redisUrl);
  else if (isProd) throw new Error("REDIS_URL é obrigatório em produção");
  else bus = new MemoryBus();

  let store: DeviceStore;
  if (env["DATABASE_URL"]) {
    const { createDb } = await import("./db/client.js");
    store = new PrismaStore(createDb(env["DATABASE_URL"]));
  } else if (isProd) {
    throw new Error("DATABASE_URL é obrigatório em produção");
  } else {
    store = new MemoryStore();
  }

  const turn: TurnProvider =
    env["CF_TURN_KEY_ID"] && env["CF_TURN_API_TOKEN"]
      ? new CloudflareTurn(env["CF_TURN_KEY_ID"], env["CF_TURN_API_TOKEN"], log)
      : new StunOnly();

  const lifetime = Number(env["WS_MAX_LIFETIME_MS"]);
  return {
    bus,
    store,
    turn,
    ...(Number.isFinite(lifetime) && lifetime > 0
      ? { hubConfig: { maxLifetimeMs: lifetime } }
      : {}),
  };
}
