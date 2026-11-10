/**
 * Métricas do vídeo recebido, calculadas a partir de `getStats()` (registro
 * `inbound-rtp` de vídeo). Taxas são a diferença entre duas amostras.
 */

export interface VideoSample {
  at: number;
  bytesReceived: number;
  framesDecoded: number;
  framesDropped: number;
  packetsLost: number;
  pliCount: number;
  frameWidth?: number;
  frameHeight?: number;
  jitterBufferDelay: number;
  jitterBufferEmittedCount: number;
  totalDecodeTime: number;
  decoderImplementation?: string;
}

export interface VideoMetrics {
  fps: number;
  kbps: number;
  width?: number;
  height?: number;
  framesDecoded: number;
  packetsLost: number;
  pliCount: number;
  /** Atraso médio no buffer de jitter no intervalo, em ms. */
  jitterMs?: number;
  /** Tempo médio de decodificação por quadro no intervalo, em ms. */
  decodeMs?: number;
  decoder?: string;
}

interface InboundRtp {
  type: string;
  kind?: string;
  bytesReceived?: number;
  framesDecoded?: number;
  framesDropped?: number;
  packetsLost?: number;
  pliCount?: number;
  frameWidth?: number;
  frameHeight?: number;
  jitterBufferDelay?: number;
  jitterBufferEmittedCount?: number;
  totalDecodeTime?: number;
  decoderImplementation?: string;
}

export function sampleFromStats(stats: RTCStatsReport, at: number): VideoSample | null {
  let found: InboundRtp | undefined;
  stats.forEach((s: InboundRtp) => {
    if (s.type === "inbound-rtp" && s.kind === "video") found = s;
  });
  if (!found) return null;
  return {
    at,
    bytesReceived: found.bytesReceived ?? 0,
    framesDecoded: found.framesDecoded ?? 0,
    framesDropped: found.framesDropped ?? 0,
    packetsLost: found.packetsLost ?? 0,
    pliCount: found.pliCount ?? 0,
    frameWidth: found.frameWidth,
    frameHeight: found.frameHeight,
    jitterBufferDelay: found.jitterBufferDelay ?? 0,
    jitterBufferEmittedCount: found.jitterBufferEmittedCount ?? 0,
    totalDecodeTime: found.totalDecodeTime ?? 0,
    decoderImplementation: found.decoderImplementation,
  };
}

export function metricsBetween(prev: VideoSample | null, cur: VideoSample): VideoMetrics {
  const base: VideoMetrics = {
    fps: 0,
    kbps: 0,
    width: cur.frameWidth,
    height: cur.frameHeight,
    framesDecoded: cur.framesDecoded,
    packetsLost: cur.packetsLost,
    pliCount: cur.pliCount,
    decoder: cur.decoderImplementation,
  };
  if (!prev || cur.at <= prev.at) return base;
  const secs = (cur.at - prev.at) / 1000;
  const frames = cur.framesDecoded - prev.framesDecoded;
  const emitted = cur.jitterBufferEmittedCount - prev.jitterBufferEmittedCount;
  return {
    ...base,
    fps: Math.max(0, Math.round(frames / secs)),
    kbps: Math.max(0, Math.round(((cur.bytesReceived - prev.bytesReceived) * 8) / secs / 1000)),
    jitterMs:
      emitted > 0
        ? Math.round(((cur.jitterBufferDelay - prev.jitterBufferDelay) / emitted) * 1000)
        : undefined,
    decodeMs:
      frames > 0
        ? Math.round(((cur.totalDecodeTime - prev.totalDecodeTime) / frames) * 1000 * 10) / 10
        : undefined,
  };
}

/** Lê uma amostra da conexão. */
export async function sampleVideo(pc: RTCPeerConnection): Promise<VideoSample | null> {
  return sampleFromStats(await pc.getStats(), performance.now());
}
