# Notas para sessões de desenvolvimento

- Idioma: comunicação, comentários de código e textos de UI em **português do Brasil**; identificadores em inglês.
- Plano e decisões: `docs/ARCHITECTURE.md`. Seguir a ordem das etapas; uma etapa por PR.
- Versões JS ficam no `catalog:` do `pnpm-workspace.yaml`. TypeScript fixado em 6.0.x (typescript-eslint exige < 6.1).
- Toda lógica específica de Windows vai em crates `crates/*-win`, atrás de traits do `kr-core`, para que o resto compile/teste em Linux.

## Ambiente remoto (Linux)

- O rustup não consegue baixar toolchains aqui: use `RUSTUP_TOOLCHAIN=stable` ao rodar cargo localmente.
- Código `cfg(windows)` pode ser checado aqui (tipos/assinaturas Win32, sem gerar binário): `RUSTUP_TOOLCHAIN=stable scripts/check-windows-on-linux.sh -p kr-capture-win -p kr-codec-win -p kr-input-win -p kr-identity -p kr-desk --all-targets` (o `hwnd()` do Tauri não aparece nessa checagem; erro esperado).
- `kr-codec-soft` (OpenH264) é só para testes no Linux; o app usa Media Foundation (`kr-codec-win`).
- Para compilar o crate Tauri (`kr-desk`) no Linux, instale antes: `apt-get install -y libwebkit2gtk-4.1-dev libgtk-3-dev libayatana-appindicator3-dev librsvg2-dev libxdo-dev`. Código `cfg(windows)` (DPAPI, crates `*-win`) só compila no CI Windows.
- Teste do cliente Rust contra o servidor real: suba `PORT=3100 pnpm --filter @kr-desk/server start` e rode `KR_SIGNALING_URL=ws://127.0.0.1:3100/ws cargo test -p kr-signaling -- --include-ignored`.
- `pnpm test:p2p`: host Rust (webrtc-rs) × `ViewerPeer` num Chrome headless (precisa de google-chrome; `CHROME_PATH` para outro caminho).
- Checagens locais antes de abrir PR:
  `pnpm format:check && pnpm lint && pnpm typecheck && pnpm test && RUSTUP_TOOLCHAIN=stable cargo clippy -p kr-proto -p kr-core -p kr-identity -p kr-signaling -p kr-transport -p kr-codec-soft --all-targets -- -D warnings && RUSTUP_TOOLCHAIN=stable cargo test -p kr-proto -p kr-core -p kr-identity -p kr-signaling -p kr-transport -p kr-codec-soft && pnpm test:p2p`

## Segurança do repositório (público)

- Nunca commitar `.env`, chaves, tokens ou dados pessoais. Segredos só na Vercel / GitHub Secrets.
- Workflows: `permissions: contents: read` por padrão; nunca usar `pull_request_target`.
