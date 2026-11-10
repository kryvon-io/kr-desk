// Executado pela Vercel antes do deploy (script "vercel-build").
// Aplica as migrações do banco somente em deploys de PRODUÇÃO: os previews
// compartilham o mesmo banco e não devem alterar o schema.
import { spawnSync } from "node:child_process";

const env = process.env.VERCEL_ENV ?? "local";
const branch = process.env.VERCEL_GIT_COMMIT_REF ?? "";

// Exige produção E a branch main: o primeiro deploy de um projeto pode ser
// marcado como produção mesmo vindo de outra branch.
if (env !== "production" || branch !== "main") {
  console.log(`[vercel-build] ambiente "${env}", branch "${branch}": migrações ignoradas.`);
  process.exit(0);
}

if (!process.env.DATABASE_URL_UNPOOLED && !process.env.DATABASE_URL) {
  console.error("[vercel-build] DATABASE_URL ausente; abortando o deploy.");
  process.exit(1);
}

console.log("[vercel-build] aplicando migrações (prisma db migrate)...");
const r = spawnSync("pnpm", ["exec", "prisma", "db", "migrate", "--yes", "--no-interactive"], {
  stdio: "inherit",
  env: { ...process.env, PRISMA_TELEMETRY_DISABLED: "1" },
});
process.exit(r.status ?? 1);
