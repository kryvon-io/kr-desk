import Fastify, { type FastifyServerOptions } from "fastify";

/** Mantém em sincronia com `kr_proto::PROTOCOL_VERSION`. */
export const PROTOCOL_VERSION = 1;
export const SERVICE_VERSION = "0.1.0";

/**
 * Monta a aplicação sem iniciar a escuta, para ser usada tanto pelo
 * entrypoint (`src/index.ts`, detectado pela Vercel) quanto pelos testes.
 */
export function buildApp(options: FastifyServerOptions = {}) {
  const app = Fastify(options);

  app.get("/health", async () => ({
    status: "ok" as const,
    service: "kr-desk-signaling",
    version: SERVICE_VERSION,
    protocolVersion: PROTOCOL_VERSION,
  }));

  return app;
}
