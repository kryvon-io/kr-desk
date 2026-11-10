#!/usr/bin/env -S node
import type { Contract as End } from "../../snapshots/bf32e58754286651e1347c02137aed260479a4793840901c6d0b2f918798b565/contract";
import endContract from "../../snapshots/bf32e58754286651e1347c02137aed260479a4793840901c6d0b2f918798b565/contract.json" with { type: "json" };
import { Migration, MigrationCLI, col, fn, primaryKey, sql } from "@prisma/orm-postgres/migration";

export default class M extends Migration<never, End> {
  override readonly endContractJson = endContract;

  override get operations() {
    return [
      this.createSchema({ schema: "public" }),
      this.createTable({
        schema: "public",
        table: "devices",
        columns: [
          col("appVersion", "text", { notNull: true, codecRef: { codecId: "pg/text@1" } }),
          col("createdAt", "timestamptz(3)", {
            notNull: true,
            default: fn(sql`now()`),
            codecRef: { codecId: "pg/timestamptz-date@1", typeParams: { precision: 3 } },
          }),
          col("id", "text", { notNull: true, codecRef: { codecId: "pg/text@1" } }),
          col("lastSeenAt", "timestamptz(3)", {
            notNull: true,
            default: fn(sql`now()`),
            codecRef: { codecId: "pg/timestamptz-date@1", typeParams: { precision: 3 } },
          }),
          col("name", "text", { notNull: true, codecRef: { codecId: "pg/text@1" } }),
          col("publicKey", "text", { notNull: true, codecRef: { codecId: "pg/text@1" } }),
        ],
        constraints: [primaryKey(["id"])],
      }),
      this.createTable({
        schema: "public",
        table: "session_logs",
        columns: [
          col("acceptedAt", "timestamptz(3)", {
            codecRef: { codecId: "pg/timestamptz-date@1", typeParams: { precision: 3 } },
          }),
          col("createdAt", "timestamptz(3)", {
            notNull: true,
            default: fn(sql`now()`),
            codecRef: { codecId: "pg/timestamptz-date@1", typeParams: { precision: 3 } },
          }),
          col("endReason", "text", { codecRef: { codecId: "pg/text@1" } }),
          col("endedAt", "timestamptz(3)", {
            codecRef: { codecId: "pg/timestamptz-date@1", typeParams: { precision: 3 } },
          }),
          col("hostId", "text", { notNull: true, codecRef: { codecId: "pg/text@1" } }),
          col("id", "text", { notNull: true, codecRef: { codecId: "pg/text@1" } }),
          col("status", "text", { notNull: true, codecRef: { codecId: "pg/text@1" } }),
          col("viewerId", "text", { notNull: true, codecRef: { codecId: "pg/text@1" } }),
        ],
        constraints: [primaryKey(["id"])],
      }),
      this.addUnique({
        schema: "public",
        table: "devices",
        constraint: "devices_publicKey_key",
        columns: ["publicKey"],
      }),
      this.createIndex({
        schema: "public",
        table: "session_logs",
        index: "session_logs_hostId_idx_05205577",
        columns: ["hostId"],
      }),
      this.createIndex({
        schema: "public",
        table: "session_logs",
        index: "session_logs_viewerId_idx_11cd1fa3",
        columns: ["viewerId"],
      }),
    ];
  }
}

MigrationCLI.run(import.meta.url, M);
