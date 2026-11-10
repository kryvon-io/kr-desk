import { describe, expect, it, vi } from "vitest";

import { CloudflareTurn, PUBLIC_STUN } from "../src/turn.js";

const log = { warn: vi.fn() };

function fakeFetch(status: number, body: unknown) {
  return vi.fn(
    async () => new Response(JSON.stringify(body), { status }),
  ) as unknown as typeof fetch;
}

describe("CloudflareTurn", () => {
  it("chama a API com o token e remove URLs na porta 53", async () => {
    const f = fakeFetch(201, {
      iceServers: [
        { urls: ["stun:stun.cloudflare.com:3478", "stun:stun.cloudflare.com:53"] },
        {
          urls: [
            "turn:turn.cloudflare.com:3478?transport=udp",
            "turn:turn.cloudflare.com:53?transport=udp",
            "turns:turn.cloudflare.com:443?transport=tcp",
          ],
          username: "u",
          credential: "c",
        },
      ],
    });
    const turn = new CloudflareTurn("key-id", "secret-token", log, 3600, f);
    const servers = await turn.iceServers();

    expect(f).toHaveBeenCalledWith(
      "https://rtc.live.cloudflare.com/v1/turn/keys/key-id/credentials/generate-ice-servers",
      expect.objectContaining({
        method: "POST",
        headers: expect.objectContaining({ Authorization: "Bearer secret-token" }),
        body: JSON.stringify({ ttl: 3600 }),
      }),
    );
    expect(servers).toEqual([
      { urls: ["stun:stun.cloudflare.com:3478"] },
      {
        urls: [
          "turn:turn.cloudflare.com:3478?transport=udp",
          "turns:turn.cloudflare.com:443?transport=tcp",
        ],
        username: "u",
        credential: "c",
      },
    ]);
  });

  it("em erro da API cai para STUN público sem lançar", async () => {
    const turn = new CloudflareTurn("k", "t", log, 3600, fakeFetch(401, { error: "nope" }));
    expect(await turn.iceServers()).toEqual(PUBLIC_STUN);
    expect(log.warn).toHaveBeenCalled();
  });
});
