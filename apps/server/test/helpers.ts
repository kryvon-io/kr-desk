import { generateKeyPairSync, randomUUID, sign, type KeyObject } from "node:crypto";
import type { AddressInfo } from "node:net";

import WebSocket from "ws";

import { buildApp, type AppDeps } from "../src/build-app.js";
import { AUTH_CONTEXT, PROTOCOL_VERSION } from "../src/protocol.js";

export type Msg = { type: string; [k: string]: unknown };

/** Par de chaves Ed25519 de teste, com a pública em base64url (32 bytes). */
export function newIdentity() {
  const { publicKey, privateKey } = generateKeyPairSync("ed25519");
  const jwk = publicKey.export({ format: "jwk" });
  return { publicKey: jwk.x as string, privateKey };
}

export function signNonce(privateKey: KeyObject, nonce: string) {
  return sign(null, Buffer.from(AUTH_CONTEXT + nonce, "utf8"), privateKey).toString("base64url");
}

export async function startServer(deps: AppDeps) {
  const app = await buildApp(deps, { logger: false });
  await app.listen({ port: 0, host: "127.0.0.1" });
  const { port } = app.server.address() as AddressInfo;
  return { app, url: `ws://127.0.0.1:${port}/ws` };
}

/** Cliente de teste: fila de mensagens recebidas com espera por tipo. */
export class TestClient {
  readonly ws: WebSocket;
  private readonly inbox: Msg[] = [];
  private waiters: Array<{ type: string; resolve: (m: Msg) => void }> = [];
  readonly closed: Promise<{ code: number; reason: string }>;
  readonly opened: Promise<void>;

  constructor(url: string) {
    this.ws = new WebSocket(url);
    this.opened = new Promise((res, rej) => {
      this.ws.once("open", () => res());
      this.ws.once("error", rej);
    });
    this.closed = new Promise((res) =>
      this.ws.once("close", (code, reason) => res({ code, reason: reason.toString() })),
    );
    this.ws.on("message", (data) => {
      const msg = JSON.parse(data.toString()) as Msg & { v: number };
      if (msg.v !== PROTOCOL_VERSION) throw new Error(`versão inesperada: ${msg.v}`);
      const i = this.waiters.findIndex((w) => w.type === msg.type);
      if (i >= 0) this.waiters.splice(i, 1)[0]!.resolve(msg);
      else this.inbox.push(msg);
    });
  }

  send(msg: Record<string, unknown>) {
    this.ws.send(JSON.stringify({ v: PROTOCOL_VERSION, ...msg }));
  }

  sendRaw(raw: string) {
    this.ws.send(raw);
  }

  /** Espera a próxima mensagem do tipo dado (ou já recebida e ainda não lida). */
  next<T extends Msg = Msg>(type: string, timeoutMs = 2000): Promise<T> {
    const i = this.inbox.findIndex((m) => m.type === type);
    if (i >= 0) return Promise.resolve(this.inbox.splice(i, 1)[0] as T);
    return new Promise<T>((resolve, reject) => {
      const waiter = { type, resolve: resolve as (m: Msg) => void };
      this.waiters.push(waiter);
      setTimeout(() => {
        this.waiters = this.waiters.filter((w) => w !== waiter);
        reject(new Error(`timeout esperando "${type}"; recebidas: ${JSON.stringify(this.inbox)}`));
      }, timeoutMs).unref();
    });
  }

  /** Garante que nenhuma mensagem do tipo chegue dentro da janela. */
  async expectNone(type: string, windowMs = 150) {
    await new Promise((r) => setTimeout(r, windowMs));
    const got = this.inbox.find((m) => m.type === type);
    if (got) throw new Error(`mensagem inesperada: ${JSON.stringify(got)}`);
  }

  close() {
    this.ws.close();
  }
}

/** Conecta e autentica; retorna o cliente e o ID atribuído. */
export async function connectDevice(url: string, identity = newIdentity(), name = "PC de teste") {
  const client = new TestClient(url);
  await client.opened;
  const hello = await client.next<Msg & { nonce: string }>("hello");
  client.send({
    type: "auth",
    public_key: identity.publicKey,
    signature: signNonce(identity.privateKey, hello.nonce),
    device_name: name,
    app_version: "0.1.0",
  });
  const ok = await client.next<Msg & { device_id: string }>("auth_ok");
  return { client, deviceId: ok.device_id, identity };
}

export const newRequestId = () => randomUUID();
