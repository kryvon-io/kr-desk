// Entrypoint detectado automaticamente pela Vercel (src/index.ts).
import { buildApp } from "./build-app.js";
import { depsFromEnv } from "./config.js";

const bootLog = {
  warn: (obj: object, msg: string) => console.warn(msg, obj),
};

const app = await buildApp(await depsFromEnv(process.env, bootLog), {
  logger: { level: process.env["LOG_LEVEL"] ?? "info" },
});
const port = Number(process.env["PORT"] ?? 3000);

try {
  await app.listen({ port, host: "0.0.0.0" });
} catch (err) {
  app.log.error(err);
  process.exit(1);
}
