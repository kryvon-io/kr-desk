import type { IceServer } from "./protocol.js";

export interface TurnProvider {
  /** Servidores ICE para uma sessão. Nunca lança: em falha, cai para STUN. */
  iceServers(): Promise<IceServer[]>;
}

export const PUBLIC_STUN: IceServer[] = [{ urls: ["stun:stun.cloudflare.com:3478"] }];

/** Sem TURN configurado: só STUN (conexão direta, sem relay). */
export class StunOnly implements TurnProvider {
  async iceServers() {
    return PUBLIC_STUN;
  }
}

export interface Logger {
  warn(obj: object, msg: string): void;
}

/**
 * Cloudflare Realtime TURN: gera credenciais efêmeras por sessão.
 * https://developers.cloudflare.com/realtime/turn/generate-credentials/
 */
export class CloudflareTurn implements TurnProvider {
  constructor(
    private readonly keyId: string,
    private readonly apiToken: string,
    private readonly log: Logger,
    private readonly ttlSec = 12 * 60 * 60,
    private readonly fetchImpl: typeof fetch = fetch,
  ) {}

  async iceServers(): Promise<IceServer[]> {
    try {
      const res = await this.fetchImpl(
        `https://rtc.live.cloudflare.com/v1/turn/keys/${encodeURIComponent(this.keyId)}/credentials/generate-ice-servers`,
        {
          method: "POST",
          headers: {
            Authorization: `Bearer ${this.apiToken}`,
            "Content-Type": "application/json",
          },
          body: JSON.stringify({ ttl: this.ttlSec }),
          signal: AbortSignal.timeout(4000),
        },
      );
      if (!res.ok) throw new Error(`HTTP ${res.status}`);
      const body = (await res.json()) as { iceServers?: IceServer[] | IceServer };
      const list = Array.isArray(body.iceServers)
        ? body.iceServers
        : body.iceServers
          ? [body.iceServers]
          : [];
      const cleaned = list
        .map((s) => ({
          ...s,
          urls: (Array.isArray(s.urls) ? s.urls : [s.urls])
            // A porta 53 é bloqueada por navegadores (WebView2) e só causa espera.
            .filter((u) => !/:53(\?|$)/.test(u)),
        }))
        .filter((s) => s.urls.length > 0);
      if (cleaned.length === 0) throw new Error("resposta sem iceServers");
      return cleaned;
    } catch (err) {
      this.log.warn({ err: String(err) }, "falha ao gerar credenciais TURN; usando só STUN");
      return PUBLIC_STUN;
    }
  }
}
