// Entrypoint detectado automaticamente pela Vercel (src/index.ts) e usado
// também na imagem Docker (VPS/Coolify).
// A Vercel exige que este arquivo importe o `fastify` diretamente, e o
// `listen` deve ser chamado já na carga do módulo: a inicialização assíncrona
// (Redis, banco) fica dentro de um plugin, que o Fastify aguarda antes de servir.
import Fastify from "fastify";

import { configureApp, SERVER_OPTIONS } from "./build-app.js";
import { depsFromEnv, trustProxyFromEnv } from "./config.js";

const fastify = Fastify({
  ...SERVER_OPTIONS,
  trustProxy: trustProxyFromEnv(process.env),
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

// Docker/Coolify param o contêiner com SIGTERM: fecha os WebSockets com 1001
// (os apps reconectam na hora, na instância nova) e as conexões com Redis/banco.
for (const signal of ["SIGTERM", "SIGINT"] as const) {
  process.once(signal, () => {
    fastify.log.info({ signal }, "encerrando");
    const force = setTimeout(() => process.exit(0), 10_000);
    force.unref();
    fastify.close().then(
      () => process.exit(0),
      () => process.exit(1),
    );
  });
}
