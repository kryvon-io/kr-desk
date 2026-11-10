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

function startHost() {
  return new Promise((resolve, reject) => {
    const proc = spawn("cargo", ["run", "-q", "-p", "kr-transport", "--example", "interop_host"], {
      cwd: root,
      stdio: ["ignore", "pipe", "inherit"],
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

const { proc, port, lines } = await startHost();
const hostDone = new Promise((r) => proc.on("exit", (code) => r(code)));

const browser = await chromium.launch({ executablePath: findChrome(), headless: true });
const page = await browser.newPage();
page.on("console", (m) => console.log(`[chrome] ${m.text()}`));
await page.goto(`http://127.0.0.1:${pagePort}/?ws=ws://127.0.0.1:${port}`);

let viewer;
try {
  await page.waitForFunction(() => window.__interop.ok || window.__interop.error, null, {
    timeout: 30_000,
  });
  viewer = await page.evaluate(() => window.__interop);
} catch {
  viewer = await page.evaluate(() => window.__interop);
}
const hostCode = await hostDone;
await browser.close();
server.close();

const hostLine = lines.find((l) => l.startsWith("INTEROP_"));
console.log("host  :", hostLine ?? "(sem resultado)");
console.log("viewer:", JSON.stringify(viewer));
if (hostCode !== 0 || !viewer?.ok) {
  console.error("FALHOU: interoperabilidade WebRTC Rust × Chrome");
  process.exit(1);
}
console.log("OK: Rust (webrtc-rs) e Chrome conectaram ponto a ponto e mediram a latência.");
