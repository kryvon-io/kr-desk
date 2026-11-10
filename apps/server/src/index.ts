// Entrypoint detectado automaticamente pela Vercel (src/index.ts).
import { buildApp } from "./build-app.js";

const app = buildApp({ logger: true });
const port = Number(process.env.PORT ?? 3000);

try {
  await app.listen({ port, host: "0.0.0.0" });
} catch (err) {
  app.log.error(err);
  process.exit(1);
}
