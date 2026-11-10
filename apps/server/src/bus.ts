/**
 * Estado compartilhado entre instâncias do servidor (a Vercel pode rodar várias,
 * e dois clientes podem cair em instâncias diferentes). Em produção é o Redis;
 * em testes/dev local, `MemoryBus` (que pode compartilhar um `MemoryHub` para
 * simular várias instâncias no mesmo processo).
 */
import { Redis } from "ioredis";

export type Unsubscribe = () => Promise<void>;

export interface Bus {
  publish(channel: string, message: string): Promise<void>;
  subscribe(channel: string, handler: (message: string) => void): Promise<Unsubscribe>;
  /** Grava com expiração (segundos). */
  set(key: string, value: string, ttlSec: number): Promise<void>;
  get(key: string): Promise<string | null>;
  del(key: string): Promise<void>;
  /** Incrementa um contador; na primeira vez define a expiração. Retorna o novo valor. */
  incr(key: string, ttlSec: number): Promise<number>;
  close(): Promise<void>;
}

// ---------------------------------------------------------------------------
// Memória
// ---------------------------------------------------------------------------

/** Armazenamento compartilhado por várias `MemoryBus` (simula um Redis). */
export class MemoryHub {
  readonly kv = new Map<string, { value: string; expiresAt: number }>();
  readonly channels = new Map<string, Set<(m: string) => void>>();

  read(key: string): string | null {
    const e = this.kv.get(key);
    if (!e) return null;
    if (e.expiresAt <= Date.now()) {
      this.kv.delete(key);
      return null;
    }
    return e.value;
  }
}

export class MemoryBus implements Bus {
  private readonly mine = new Set<{ channel: string; handler: (m: string) => void }>();

  constructor(private readonly hub: MemoryHub = new MemoryHub()) {}

  async publish(channel: string, message: string) {
    // Entrega assíncrona, como no Redis.
    for (const h of this.hub.channels.get(channel) ?? []) queueMicrotask(() => h(message));
  }

  async subscribe(channel: string, handler: (m: string) => void): Promise<Unsubscribe> {
    let set = this.hub.channels.get(channel);
    if (!set) this.hub.channels.set(channel, (set = new Set()));
    set.add(handler);
    const entry = { channel, handler };
    this.mine.add(entry);
    return async () => {
      this.hub.channels.get(channel)?.delete(handler);
      this.mine.delete(entry);
    };
  }

  async set(key: string, value: string, ttlSec: number) {
    this.hub.kv.set(key, { value, expiresAt: Date.now() + ttlSec * 1000 });
  }

  async get(key: string) {
    return this.hub.read(key);
  }

  async del(key: string) {
    this.hub.kv.delete(key);
  }

  async incr(key: string, ttlSec: number) {
    const current = this.hub.read(key);
    const next = (current ? Number(current) : 0) + 1;
    const expiresAt = current
      ? (this.hub.kv.get(key)?.expiresAt ?? Date.now() + ttlSec * 1000)
      : Date.now() + ttlSec * 1000;
    this.hub.kv.set(key, { value: String(next), expiresAt });
    return next;
  }

  async close() {
    for (const { channel, handler } of this.mine) this.hub.channels.get(channel)?.delete(handler);
    this.mine.clear();
  }
}

// ---------------------------------------------------------------------------
// Redis (Upstash em produção)
// ---------------------------------------------------------------------------

export class RedisBus implements Bus {
  private readonly cmd: Redis;
  private readonly sub: Redis;
  private readonly handlers = new Map<string, Set<(m: string) => void>>();

  constructor(url: string) {
    const opts = { maxRetriesPerRequest: 3, enableAutoPipelining: true, lazyConnect: false };
    this.cmd = new Redis(url, opts);
    this.sub = new Redis(url, opts);
    this.sub.on("message", (channel: string, message: string) => {
      for (const h of this.handlers.get(channel) ?? []) h(message);
    });
  }

  async publish(channel: string, message: string) {
    await this.cmd.publish(channel, message);
  }

  async subscribe(channel: string, handler: (m: string) => void): Promise<Unsubscribe> {
    let set = this.handlers.get(channel);
    if (!set) {
      this.handlers.set(channel, (set = new Set()));
      await this.sub.subscribe(channel);
    }
    set.add(handler);
    return async () => {
      const s = this.handlers.get(channel);
      if (!s) return;
      s.delete(handler);
      if (s.size === 0) {
        this.handlers.delete(channel);
        await this.sub.unsubscribe(channel);
      }
    };
  }

  async set(key: string, value: string, ttlSec: number) {
    await this.cmd.set(key, value, "EX", ttlSec);
  }

  async get(key: string) {
    return this.cmd.get(key);
  }

  async del(key: string) {
    await this.cmd.del(key);
  }

  async incr(key: string, ttlSec: number) {
    const [[, value]] = (await this.cmd.multi().incr(key).expire(key, ttlSec, "NX").exec()) as [
      [Error | null, number],
      unknown,
    ];
    return value;
  }

  async close() {
    await Promise.allSettled([this.cmd.quit(), this.sub.quit()]);
  }
}
