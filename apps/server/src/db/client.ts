// Node 24 (runtime da Vercel) ainda não tem `Temporal`; o Prisma 8 precisa dele.
import "temporal-polyfill/global";
import postgres from "@prisma/orm-postgres/runtime";
import { contractFromJson } from "@prisma/orm-postgres/static";

import type { Contract } from "./contract.js";
import contractJson from "./contract.json" with { type: "json" };

export const contract = contractFromJson<Contract>(contractJson);

export function createDb(url: string) {
  return postgres({ contract, url });
}

export type Db = ReturnType<typeof createDb>;
