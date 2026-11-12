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

## Baixar

- **Versão publicada:** [última release](https://github.com/kryvon-io/kr-desk/releases/latest) → `KR Desk_<versão>_x64-setup.exe`. Depois de instalado, o app avisa sozinho quando sai versão nova ("Atualizar e reiniciar").
- **Build de teste:** cada execução do workflow **CI** gera o artefato `kr-desk-windows-x64-nsis` (aba _Actions_ → execução → _Artifacts_).

As builds ainda **não são assinadas** (Authenticode): o SmartScreen exibirá um aviso — use "Mais informações → Executar assim mesmo".

### Publicar uma versão

1. Suba a versão em `Cargo.toml`, `apps/desktop/package.json` e `apps/desktop/src-tauri/tauri.conf.json` (num PR).
2. Depois do merge, crie **só a tag**: `git tag v<versão> && git push origin v<versão>`. Não publique uma release à mão: o workflow cria a dele, e duas releases com a mesma tag confundem a atualização.
3. O workflow **Release (Windows)** gera o instalador, o pacote assinado da atualização e o `latest.json` num **rascunho** de release.
4. Revise o rascunho e clique em **Publish release**: a partir daí, os apps instalados recebem a atualização.

A atualização automática exige os segredos `TAURI_SIGNING_PRIVATE_KEY` e `TAURI_SIGNING_PRIVATE_KEY_PASSWORD` no repositório.

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
