/**
 * Persistência durável (dispositivos e auditoria de sessões).
 * Presença e roteamento NÃO ficam aqui (ver `bus.ts`).
 */
import type { Db } from "./db/client.js";

export interface Device {
  id: string;
  publicKey: string;
  name: string;
  appVersion: string;
}

export type SessionStatus = "pending" | "accepted" | "rejected" | "ended";

export interface DeviceStore {
  findByPublicKey(publicKey: string): Promise<Device | null>;
  /** Cria o dispositivo. Retorna null se o `id` já existir (o chamador tenta outro). */
  create(device: Device): Promise<Device | null>;
  touch(id: string, name: string, appVersion: string): Promise<void>;
  logSession(id: string, hostId: string, viewerId: string): Promise<void>;
  updateSession(id: string, status: SessionStatus, reason?: string): Promise<void>;
  close(): Promise<void>;
}

// ---------------------------------------------------------------------------

export class MemoryStore implements DeviceStore {
  readonly devices = new Map<string, Device>();
  readonly sessions = new Map<
    string,
    { hostId: string; viewerId: string; status: SessionStatus; reason?: string }
  >();

  async findByPublicKey(publicKey: string) {
    for (const d of this.devices.values()) if (d.publicKey === publicKey) return { ...d };
    return null;
  }

  async create(device: Device) {
    if (this.devices.has(device.id)) return null;
    for (const d of this.devices.values()) {
      if (d.publicKey === device.publicKey) throw new Error("publicKey duplicada");
    }
    this.devices.set(device.id, { ...device });
    return { ...device };
  }

  async touch(id: string, name: string, appVersion: string) {
    const d = this.devices.get(id);
    if (d) Object.assign(d, { name, appVersion });
  }

  async logSession(id: string, hostId: string, viewerId: string) {
    this.sessions.set(id, { hostId, viewerId, status: "pending" });
  }

  async updateSession(id: string, status: SessionStatus, reason?: string) {
    const s = this.sessions.get(id);
    if (s) Object.assign(s, { status }, reason ? { reason } : {});
  }

  async close() {}
}

// ---------------------------------------------------------------------------

const UNIQUE_VIOLATION = "23505";

function isUniqueViolation(err: unknown): boolean {
  // O erro do driver `pg` pode vir embrulhado pelo Prisma; procura o código na cadeia.
  for (let e: unknown = err, depth = 0; e && depth < 5; depth++) {
    const o = e as { code?: unknown; cause?: unknown };
    if (o.code === UNIQUE_VIOLATION) return true;
    if (typeof o.code === "string" && o.code.includes("UNIQUE")) return true;
    e = o.cause;
  }
  return /unique|duplicate key/i.test(String(err));
}

export class PrismaStore implements DeviceStore {
  constructor(private readonly db: Db) {}

  private get m() {
    return this.db.orm.public;
  }

  async findByPublicKey(publicKey: string) {
    const d = await this.m.Device.whereUnique({ publicKey }).first();
    return d ? { id: d.id, publicKey: d.publicKey, name: d.name, appVersion: d.appVersion } : null;
  }

  async create(device: Device) {
    try {
      await this.m.Device.create({ ...device, lastSeenAt: new Date() });
      return device;
    } catch (err) {
      if (!isUniqueViolation(err)) throw err;
      // Colisão de ID (rara) → o chamador tenta outro. Colisão de chave → corrida
      // entre dois logins do mesmo dispositivo: devolve o existente.
      const existing = await this.findByPublicKey(device.publicKey);
      if (existing) return existing;
      return null;
    }
  }

  async touch(id: string, name: string, appVersion: string) {
    await this.m.Device.where({ id }).update({ name, appVersion, lastSeenAt: new Date() });
  }

  async logSession(id: string, hostId: string, viewerId: string) {
    await this.m.SessionLog.create({ id, hostId, viewerId, status: "pending" });
  }

  async updateSession(id: string, status: SessionStatus, reason?: string) {
    const now = new Date();
    await this.m.SessionLog.where({ id }).update({
      status,
      ...(status === "accepted" ? { acceptedAt: now } : {}),
      ...(status === "ended" || status === "rejected" ? { endedAt: now } : {}),
      ...(reason ? { endReason: reason } : {}),
    });
  }

  async close() {
    await this.db.close();
  }
}
