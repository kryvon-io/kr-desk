import { MemoryBus, RedisBus, type Bus } from "./bus.js";
import type { HubConfig } from "./hub.js";
import type { AppDeps } from "./build-app.js";
import { MemoryStore, PrismaStore, type DeviceStore } from "./store.js";
import { CloudflareTurn, StunOnly, type Logger, type TurnProvider } from "./turn.js";

/**
 * Monta as dependências a partir das variáveis de ambiente.
 * Sem banco/Redis configurados (dev local), cai para implementações em memória.
 */
export async function depsFromEnv(env: NodeJS.ProcessEnv, log: Logger): Promise<AppDeps> {
  const isProd = env["VERCEL_ENV"] === "production" || env["NODE_ENV"] === "production";
  // Numa VPS com um único processo (Coolify/Docker), o roteamento pode ficar
  // em memória: o Redis só é necessário com várias instâncias (Vercel) ou
  // durante a migração, para as duas hospedagens se enxergarem.
  const singleInstance = env["SIGNALING_SINGLE_INSTANCE"] === "1";

  // A integração Upstash da Vercel pode expor a URL com nomes diferentes.
  const redisUrl = env["REDIS_URL"] ?? env["KV_URL"] ?? env["UPSTASH_REDIS_URL"];
  let bus: Bus;
  if (redisUrl) bus = new RedisBus(redisUrl);
  else if (isProd && !singleInstance) throw new Error("REDIS_URL é obrigatório em produção");
  else bus = new MemoryBus();

  // O Prisma 8 envia parâmetros de inicialização (ex.: lc_monetary) que o pooler
  // da Neon (PgBouncer) recusa; por isso usamos a conexão direta quando houver.
  const databaseUrl = env["DATABASE_URL_UNPOOLED"] ?? env["DATABASE_URL"];
  let store: DeviceStore;
  if (databaseUrl) {
    const { createDb } = await import("./db/client.js");
    store = new PrismaStore(createDb(databaseUrl));
  } else if (isProd) {
    throw new Error("DATABASE_URL é obrigatório em produção");
  } else {
    store = new MemoryStore();
  }

  const turn: TurnProvider =
    env["CF_TURN_KEY_ID"] && env["CF_TURN_API_TOKEN"]
      ? new CloudflareTurn(env["CF_TURN_KEY_ID"], env["CF_TURN_API_TOKEN"], log)
      : new StunOnly();

  const hubConfig: Partial<HubConfig> = {};
  const lifetime = Number(env["WS_MAX_LIFETIME_MS"]);
  if (Number.isFinite(lifetime) && lifetime > 0) hubConfig.maxLifetimeMs = lifetime;
  const refresh = Number(env["PRESENCE_REFRESH_MS"]);
  if (Number.isFinite(refresh) && refresh > 0) hubConfig.presenceRefreshMs = refresh;
  return {
    bus,
    store,
    turn,
    ...(Object.keys(hubConfig).length > 0 ? { hubConfig } : {}),
  };
}

/**
 * Quantos proxies confiáveis há na frente do servidor, para achar o IP real
 * (usado no limite de tentativas). Na Vercel (`true`) a borda reescreve o
 * X-Forwarded-For; numa VPS, confiar em tudo deixaria o cliente forjar o IP.
 * `TRUST_PROXY=1` (só o proxy do Coolify) ou `2` (Cloudflare + Coolify).
 */
export function trustProxyFromEnv(
  env: NodeJS.ProcessEnv,
): boolean | ((address: string, hop: number) => boolean) {
  const raw = env["TRUST_PROXY"];
  if (raw === undefined || raw === "" || raw === "true") return true;
  if (raw === "false" || raw === "0") return false;
  const hops = Number(raw);
  if (!Number.isInteger(hops) || hops <= 0) return true;
  // Confia só nos `hops` proxies mais próximos; o resto do X-Forwarded-For
  // (que o cliente pode inventar) é ignorado.
  return (_address, hop) => hop < hops;
}
