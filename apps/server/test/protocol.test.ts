import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";

import { describe, expect, it } from "vitest";

import { verifyAuthSignature } from "../src/identity.js";
import { decode, encode, PROTOCOL_VERSION, type ServerMessage } from "../src/protocol.js";

const fixture = <T>(name: string): T =>
  JSON.parse(
    readFileSync(
      fileURLToPath(new URL(`../../../crates/kr-proto/fixtures/${name}.json`, import.meta.url)),
      "utf8",
    ),
  ) as T;

describe("compatibilidade com kr-proto (fixtures compartilhados)", () => {
  it("aceita todas as mensagens de cliente do Rust", () => {
    for (const msg of fixture<object[]>("client-messages")) {
      const r = decode(JSON.stringify(msg));
      expect(r, JSON.stringify(msg)).toMatchObject({ ok: true });
    }
  });

  it("gera mensagens de servidor no mesmo formato do Rust", () => {
    for (const msg of fixture<Array<{ v: number } & ServerMessage>>("server-messages")) {
      const { v, ...rest } = msg;
      expect(v).toBe(PROTOCOL_VERSION);
      expect(JSON.parse(encode(rest as ServerMessage))).toEqual(msg);
    }
  });

  it("verifica a assinatura do vetor gerado em Rust", () => {
    const v = fixture<{ public_key: string; nonce: string; signature: string }>("auth-vector");
    expect(verifyAuthSignature(v.public_key, v.nonce, v.signature)).toBe(true);
    expect(verifyAuthSignature(v.public_key, v.nonce + "x", v.signature)).toBe(false);
    const flipped = (v.signature[0] === "A" ? "B" : "A") + v.signature.slice(1);
    expect(verifyAuthSignature(v.public_key, v.nonce, flipped)).toBe(false);
  });
});

describe("decode", () => {
  it("rejeita JSON inválido, versão errada e campos inválidos", () => {
    expect(decode("{")).toMatchObject({ ok: false, code: "bad_message" });
    expect(decode('{"v":2,"type":"ping","nonce":1}')).toMatchObject({
      ok: false,
      code: "unsupported_version",
    });
    expect(
      decode('{"v":1,"type":"connect_request","request_id":"x","target_id":"1"}'),
    ).toMatchObject({
      ok: false,
      code: "bad_message",
    });
    expect(
      decode(
        '{"v":1,"type":"connect_request","request_id":"6f1c2b9e-3d4a-4c5b-9e8f-0a1b2c3d4e5f","target_id":"012345678"}',
      ),
    ).toMatchObject({ ok: false });
  });
});
