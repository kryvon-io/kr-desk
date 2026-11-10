import { createPublicKey, randomBytes, randomInt, verify } from "node:crypto";

import { AUTH_CONTEXT } from "./protocol.js";

/** Nonce de 16 bytes aleatórios em base64url, enviado no `hello`. */
export function newNonce(): string {
  return randomBytes(16).toString("base64url");
}

/**
 * Verifica a assinatura Ed25519 de `AUTH_CONTEXT + nonce`.
 * Retorna false para qualquer entrada malformada em vez de lançar.
 */
export function verifyAuthSignature(publicKeyB64: string, nonce: string, signatureB64: string) {
  try {
    const raw = Buffer.from(publicKeyB64, "base64url");
    const sig = Buffer.from(signatureB64, "base64url");
    if (raw.length !== 32 || sig.length !== 64) return false;
    const key = createPublicKey({
      key: { kty: "OKP", crv: "Ed25519", x: publicKeyB64 },
      format: "jwk",
    });
    return verify(null, Buffer.from(AUTH_CONTEXT + nonce, "utf8"), key, sig);
  } catch {
    return false;
  }
}

/** Gera um ID público de 9 dígitos (100000000–999999999). */
export function newDeviceId(): string {
  return String(randomInt(100_000_000, 1_000_000_000));
}
