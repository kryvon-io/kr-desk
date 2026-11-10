import { definePrismaConfig } from "@prisma/cli-engine";
import { defineConfig as ormConfig } from "@prisma/orm-postgres/config";

// Migrações usam a conexão direta (sem pooler) quando disponível.
const connection =
  process.env["DATABASE_URL_UNPOOLED"] ??
  process.env["DATABASE_URL"] ??
  "postgres://localhost:5432/krdesk";

export default definePrismaConfig({
  orm: ormConfig({
    contract: "./src/db/contract.prisma",
    db: { connection },
  }),
});
