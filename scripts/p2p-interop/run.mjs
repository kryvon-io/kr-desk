/* global window -- usado dentro de page.evaluate, que roda no Chrome */
// Teste de interoperabilidade WebRTC: host em Rust (webrtc-rs) × viewer no
// Chrome (o mesmo ViewerPeer do app). Uso: pnpm test:p2p
// Requer Google Chrome/Chromium instalado (CHROME_PATH para outro caminho).
import { spawn } from "node:child_process";
import { existsSync } from "node:fs";
import { createServer } from "node:http";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

import { build } from "esbuild";
import { chromium } from "playwright-core";

const here = dirname(fileURLToPath(import.meta.url));
const root = join(here, "..", "..");

function findChrome() {
  const candidates = [
    process.env.CHROME_PATH,
    "/usr/bin/google-chrome",
    "/usr/bin/chromium",
    "/usr/bin/chromium-browser",
    "C:\\Program Files\\Google\\Chrome\\Application\\chrome.exe",
  ].filter(Boolean);
  const found = candidates.find((p) => existsSync(p));
  if (!found) throw new Error("Chrome não encontrado; defina CHROME_PATH");
  return found;
}

function startHost(tamper) {
  return new Promise((resolve, reject) => {
    const proc = spawn("cargo", ["run", "-q", "-p", "kr-transport", "--example", "interop_host"], {
      cwd: root,
      stdio: ["ignore", "pipe", "inherit"],
      env: { ...process.env, KR_INTEROP_TAMPER: tamper ? "1" : "0" },
    });
    let buffer = "";
    const lines = [];
    proc.stdout.on("data", (chunk) => {
      buffer += chunk;
      let i;
      while ((i = buffer.indexOf("\n")) >= 0) {
        const line = buffer.slice(0, i).trim();
        buffer = buffer.slice(i + 1);
        lines.push(line);
        const m = /^LISTENING (\d+)$/.exec(line);
        if (m) resolve({ proc, port: Number(m[1]), lines });
      }
    });
    proc.on("exit", (code) => reject(new Error(`host saiu antes de escutar (código ${code})`)));
  });
}

const bundle = await build({
  entryPoints: [join(here, "page.ts")],
  bundle: true,
  write: false,
  format: "iife",
  target: "chrome120",
});
const html = `<!doctype html><meta charset="utf-8"><script>${bundle.outputFiles[0].text}</script>`;
const server = createServer((_, res) =>
  res.writeHead(200, { "content-type": "text/html" }).end(html),
);
await new Promise((r) => server.listen(0, "127.0.0.1", r));
const pagePort = server.address().port;

const browser = await chromium.launch({ executablePath: findChrome(), headless: true });

/** Executa um cenário e devolve o resultado do host e do viewer. */
async function scenario(tamper) {
  const { proc, port, lines } = await startHost(tamper);
  const hostDone = new Promise((r) => proc.on("exit", (code) => r(code)));
  const page = await browser.newPage();
  page.on("console", (m) => console.log(`[chrome] ${m.text()}`));
  await page.goto(`http://127.0.0.1:${pagePort}/?ws=ws://127.0.0.1:${port}`);
  let viewer;
  try {
    await page.waitForFunction(() => window.__interop.ok || window.__interop.error, null, {
      timeout: 30_000,
    });
  } finally {
    viewer = await page.evaluate(() => window.__interop).catch(() => undefined);
  }
  // No cenário normal, o host termina sozinho depois do "done" da página.
  if (tamper) proc.kill();
  const hostCode = await hostDone;
  // Estado final (o teste de reconexão termina depois do "ok").
  viewer = (await page.evaluate(() => window.__interop).catch(() => undefined)) ?? viewer;
  await page.close();
  return { hostCode, viewer, hostLine: lines.find((l) => l.startsWith("INTEROP_")) };
}

const normal = await scenario(false);
console.log("host  :", normal.hostLine ?? "(sem resultado)");
console.log("viewer:", JSON.stringify(normal.viewer));
// Intermediário simulado: o host assina um fingerprint que não é o dele.
const mitm = await scenario(true);
console.log("MITM  :", JSON.stringify(mitm.viewer));
await browser.close();
server.close();

let failed = false;
if (normal.hostCode !== 0 || !normal.viewer?.ok || !normal.viewer?.reconnected) {
  console.error("FALHOU: interoperabilidade WebRTC Rust × Chrome");
  failed = true;
}
if (mitm.viewer?.ok || !String(mitm.viewer?.error ?? "").startsWith("auth:")) {
  console.error("FALHOU: o viewer aceitou um host com fingerprint adulterado");
  failed = true;
}
if (failed) process.exit(1);
console.log(
  "OK: Rust (webrtc-rs) e Chrome se autenticaram, conectaram ponto a ponto, mediram a latência, o Chrome decodificou o vídeo H.264, entrada e cursor chegaram, a conexão voltou após reiniciar o ICE — e o fingerprint adulterado foi recusado.",
);
