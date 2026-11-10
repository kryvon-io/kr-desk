/**
 * Autenticação ponta a ponta da sessão — espelho de `kr_proto::session`.
 *
 * Cada lado assina os fingerprints DTLS que ele mesmo vê (o seu e o do
 * outro). Com alguém no meio, os fingerprints vistos não batem e a
 * verificação falha. O código de 6 dígitos sai dos dois fingerprints e só é
 * igual nas duas telas sem intermediário.
 */

export const SESSION_CONTEXT = "kr-desk/session/v1";
export const SAS_CONTEXT = "kr-desk/sas/v1";

export type SessionRole = "host" | "viewer";

/** `a=fingerprint:SHA-256 ab:cd` → `sha-256 AB:CD`. */
export function sdpFingerprint(sdp: string | undefined | null): string | null {
  for (const raw of (sdp ?? "").split(/\r?\n/)) {
    const line = raw.trim();
    if (!line.startsWith("a=fingerprint:")) continue;
    const value = line.slice("a=fingerprint:".length).trim();
    const space = value.indexOf(" ");
    if (space < 0) continue;
    return `${value.slice(0, space).trim().toLowerCase()} ${value
      .slice(space + 1)
      .trim()
      .toUpperCase()}`;
  }
  return null;
}

export function sessionMessage(
  requestId: string,
  signer: SessionRole,
  signerFingerprint: string,
  peerFingerprint: string,
): string {
  return `${SESSION_CONTEXT}\n${requestId}\n${signer}\n${signerFingerprint}\n${peerFingerprint}`;
}

export function sasInput(requestId: string, a: string, b: string): string {
  // Mesma ordem que `str::cmp` do Rust (comparação por código, não locale).
  const [lo, hi] = a <= b ? [a, b] : [b, a];
  return `${SAS_CONTEXT}\n${requestId}\n${lo}\n${hi}`;
}

/** Código de verificação de 6 dígitos. */
export async function verificationCode(requestId: string, a: string, b: string): Promise<string> {
  const digest = await crypto.subtle.digest(
    "SHA-256",
    new TextEncoder().encode(sasInput(requestId, a, b)),
  );
  const n = new DataView(digest).getUint32(0) % 1_000_000;
  return String(n).padStart(6, "0");
}

export function formatCode(code: string): string {
  return `${code.slice(0, 3)} ${code.slice(3)}`;
}

/** Resultado da verificação da prova do outro lado. */
export type VerifyOutcome =
  | { result: "ok"; firstContact: boolean }
  | { result: "changed" }
  | { result: "failed"; reason: string };

/** Quem assina e verifica (no app: comandos Rust; em testes: WebCrypto). */
export interface SessionCrypto {
  sign(message: string): Promise<{ publicKey: string; signature: string }>;
  verify(publicKey: string, signature: string, message: string): Promise<VerifyOutcome>;
}

export function base64UrlEncode(bytes: Uint8Array): string {
  let bin = "";
  for (const b of bytes) bin += String.fromCharCode(b);
  return btoa(bin).replace(/\+/g, "-").replace(/\//g, "_").replace(/=+$/, "");
}

export function base64UrlDecode(text: string): Uint8Array<ArrayBuffer> {
  const b64 = text.replace(/-/g, "+").replace(/_/g, "/");
  const bin = atob(b64 + "=".repeat((4 - (b64.length % 4)) % 4));
  return Uint8Array.from(bin, (c) => c.charCodeAt(0));
}
