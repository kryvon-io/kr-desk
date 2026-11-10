// Entrypoint detectado automaticamente pela Vercel (src/index.ts).
// A Vercel exige que este arquivo importe o `fastify` diretamente.
import Fastify from "fastify";

import { configureApp, SERVER_OPTIONS } from "./build-app.js";
import { depsFromEnv } from "./config.js";

const fastify = Fastify({
  ...SERVER_OPTIONS,
  logger: { level: process.env["LOG_LEVEL"] ?? "info" },
});
await configureApp(fastify, await depsFromEnv(process.env, fastify.log));

try {
  await fastify.listen({ port: Number(process.env["PORT"] ?? 3000), host: "0.0.0.0" });
} catch (err) {
  fastify.log.error(err);
  process.exit(1);
}
