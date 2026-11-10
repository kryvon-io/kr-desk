# Notas para sessões de desenvolvimento

- Idioma: comunicação, comentários de código e textos de UI em **português do Brasil**; identificadores em inglês.
- Plano e decisões: `docs/ARCHITECTURE.md`. Seguir a ordem das etapas; uma etapa por PR.
- Versões JS ficam no `catalog:` do `pnpm-workspace.yaml`. TypeScript fixado em 6.0.x (typescript-eslint exige < 6.1).
- Toda lógica específica de Windows vai em crates `crates/*-win`, atrás de traits do `kr-core`, para que o resto compile/teste em Linux.

## Ambiente remoto (Linux)

- O rustup não consegue baixar toolchains aqui: use `RUSTUP_TOOLCHAIN=stable` ao rodar cargo localmente.
- Não há webkit2gtk: o crate `kr-desk` (src-tauri) e os crates `*-win` só são compilados no CI Windows.
- Checagens locais antes de abrir PR:
  `pnpm format:check && pnpm lint && pnpm typecheck && pnpm test && RUSTUP_TOOLCHAIN=stable cargo clippy -p kr-proto -p kr-core --all-targets -- -D warnings && RUSTUP_TOOLCHAIN=stable cargo test -p kr-proto -p kr-core`

## Segurança do repositório (público)

- Nunca commitar `.env`, chaves, tokens ou dados pessoais. Segredos só na Vercel / GitHub Secrets.
- Workflows: `permissions: contents: read` por padrão; nunca usar `pull_request_target`.
