import { describe, expect, it } from "vitest";

import { metricsBetween, sampleFromStats, type VideoSample } from "./video-stats";

function report(entries: Record<string, unknown>[]): RTCStatsReport {
  const map = new Map(entries.map((e, i) => [String(i), e]));
  return map as unknown as RTCStatsReport;
}

const sample = (at: number, over: Partial<VideoSample> = {}): VideoSample => ({
  at,
  bytesReceived: 0,
  framesDecoded: 0,
  framesDropped: 0,
  packetsLost: 0,
  pliCount: 0,
  jitterBufferDelay: 0,
  jitterBufferEmittedCount: 0,
  totalDecodeTime: 0,
  ...over,
});

describe("video-stats", () => {
  it("lê o inbound-rtp de vídeo e ignora áudio/outros", () => {
    const s = sampleFromStats(
      report([
        { type: "inbound-rtp", kind: "audio", bytesReceived: 9 },
        { type: "transport" },
        {
          type: "inbound-rtp",
          kind: "video",
          bytesReceived: 1000,
          framesDecoded: 30,
          frameWidth: 1920,
          frameHeight: 1080,
          decoderImplementation: "D3D11VideoDecoder",
        },
      ]),
      5,
    );
    expect(s).toMatchObject({
      at: 5,
      bytesReceived: 1000,
      framesDecoded: 30,
      frameWidth: 1920,
      decoderImplementation: "D3D11VideoDecoder",
    });
    expect(sampleFromStats(report([{ type: "transport" }]), 0)).toBeNull();
  });

  it("calcula fps, kbps, jitter e decodificação entre amostras", () => {
    const a = sample(1000, { bytesReceived: 0, framesDecoded: 0 });
    const b = sample(2000, {
      bytesReceived: 500_000,
      framesDecoded: 30,
      jitterBufferDelay: 0.6,
      jitterBufferEmittedCount: 30,
      totalDecodeTime: 0.09,
    });
    expect(metricsBetween(a, b)).toMatchObject({ fps: 30, kbps: 4000, jitterMs: 20, decodeMs: 3 });
  });

  it("primeira amostra não tem taxa", () => {
    expect(metricsBetween(null, sample(1, { framesDecoded: 3 }))).toMatchObject({
      fps: 0,
      kbps: 0,
      framesDecoded: 3,
    });
  });
});
