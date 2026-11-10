import { describe, expect, it } from "vitest";

import vector from "../../../../crates/kr-proto/fixtures/session-vector.json";
import {
  base64UrlDecode,
  base64UrlEncode,
  formatCode,
  sdpFingerprint,
  sessionMessage,
  verificationCode,
} from "./session-auth";

describe("session-auth (vetor compartilhado com o Rust)", () => {
  it("extrai e normaliza os fingerprints", () => {
    expect(sdpFingerprint(vector.host_sdp)).toBe(vector.host_fp);
    expect(sdpFingerprint(vector.viewer_sdp)).toBe(vector.viewer_fp);
    expect(sdpFingerprint("v=0\r\n")).toBeNull();
  });

  it("monta as mensagens assinadas", () => {
    expect(sessionMessage(vector.request_id, "host", vector.host_fp, vector.viewer_fp)).toBe(
      vector.host_message,
    );
    expect(sessionMessage(vector.request_id, "viewer", vector.viewer_fp, vector.host_fp)).toBe(
      vector.viewer_message,
    );
  });

  it("calcula o mesmo código de verificação, em qualquer ordem", async () => {
    expect(await verificationCode(vector.request_id, vector.host_fp, vector.viewer_fp)).toBe(
      vector.sas,
    );
    expect(await verificationCode(vector.request_id, vector.viewer_fp, vector.host_fp)).toBe(
      vector.sas,
    );
    expect(formatCode("826257")).toBe("826 257");
  });

  it("verifica a assinatura Rust com WebCrypto (Ed25519)", async () => {
    const key = await crypto.subtle.importKey(
      "raw",
      base64UrlDecode(vector.host_public_key),
      { name: "Ed25519" },
      false,
      ["verify"],
    );
    const ok = await crypto.subtle.verify(
      { name: "Ed25519" },
      key,
      base64UrlDecode(vector.host_signature),
      new TextEncoder().encode(vector.host_message),
    );
    expect(ok).toBe(true);
    expect(base64UrlEncode(base64UrlDecode(vector.host_public_key))).toBe(vector.host_public_key);
  });
});
