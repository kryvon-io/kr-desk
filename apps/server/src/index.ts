// Entrypoint detectado automaticamente pela Vercel (src/index.ts).
// A Vercel exige que este arquivo importe o `fastify` diretamente, e o
// `listen` deve ser chamado já na carga do módulo: a inicialização assíncrona
// (Redis, banco) fica dentro de um plugin, que o Fastify aguarda antes de servir.
import Fastify from "fastify";

import { configureApp, SERVER_OPTIONS } from "./build-app.js";
import { depsFromEnv } from "./config.js";

const fastify = Fastify({
  ...SERVER_OPTIONS,
  logger: { level: process.env["LOG_LEVEL"] ?? "info" },
});

fastify.register(async (app) => {
  await configureApp(app, await depsFromEnv(process.env, app.log));
});

fastify.listen({ port: Number(process.env["PORT"] ?? 3000), host: "0.0.0.0" }, (err) => {
  if (err) {
    fastify.log.error(err);
    process.exit(1);
  }
});
