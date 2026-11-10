# KR Desk

Aplicativo de acesso remoto para Windows 11 (64 bits): conecte dois computadores pela internet com um ID único, autorização do usuário, transmissão de tela em tempo real e controle de mouse e teclado.

> 🚧 **Em desenvolvimento — Etapa 0 (fundação).** Ainda não há conexão remota funcional.

## Stack

| Camada       | Tecnologia                                                                                      |
| ------------ | ----------------------------------------------------------------------------------------------- |
| Desktop      | Tauri 2 · React 19 · TypeScript · Vite 8 · Tailwind CSS 4 · shadcn/ui · Lucide                  |
| Motor nativo | Rust · Tokio · windows-rs (DXGI Desktop Duplication, Media Foundation H.264, SendInput)         |
| Comunicação  | WebRTC (webrtc-rs 0.21 no host, WebRTC nativo do WebView2 no viewer)                            |
| Backend      | Node.js 26 · Fastify 5 · WebSocket · Prisma 8 · PostgreSQL (Neon) · Redis (Upstash) — na Vercel |
| Relay        | Cloudflare Realtime TURN                                                                        |
| CI/CD        | GitHub Actions (instalador NSIS)                                                                |

Arquitetura e plano de etapas: [`docs/ARCHITECTURE.md`](docs/ARCHITECTURE.md).

## Estrutura

```
apps/desktop      App Tauri (React em src/, Rust em src-tauri/)
apps/server       Servidor de sinalização (Fastify)
crates/kr-proto   Tipos do protocolo
crates/kr-core    Núcleo de sessão, independente de plataforma
```

## Baixar uma build de teste

Cada execução do workflow **CI** gera o artefato `kr-desk-windows-x64-nsis` (aba _Actions_ → execução → _Artifacts_). As builds ainda **não são assinadas**: o SmartScreen exibirá um aviso — use "Mais informações → Executar assim mesmo".

## Desenvolvimento

Requisitos: Node.js 26, pnpm 12, Rust 1.97 (fixado em `rust-toolchain.toml`).

```bash
pnpm install
pnpm dev:server        # servidor em http://localhost:3000/health
pnpm dev:desktop       # app desktop (Windows)
pnpm lint && pnpm typecheck && pnpm test
cargo test -p kr-proto -p kr-core
```

## Segurança

Veja [SECURITY.md](SECURITY.md) para reportar vulnerabilidades de forma privada.

## Licença

[GNU Affero General Public License v3.0](LICENSE) (AGPL-3.0-only).
