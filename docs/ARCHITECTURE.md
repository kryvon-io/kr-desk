# KR Desk — Validação de stack, arquitetura e plano de etapas

> Status (10/11/2026): Etapas 0–5 concluídas · seleção de monitor (complemento da Etapa 5) em PR. Repositório público `kryvon-io/kr-desk` (AGPL-3.0).

---

## 1. Validação de versões e compatibilidade

| Componente               | Versão recomendada                                                                 | Situação                                   | Observação                                                                                                                                                                       |
| ------------------------ | ---------------------------------------------------------------------------------- | ------------------------------------------ | -------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Tauri                    | 2.11.x                                                                             | Estável                                    | Usa WebView2 no Windows 11 (já vem instalado).                                                                                                                                   |
| React / TypeScript       | React 19.2 / TS estável mais recente                                               | Estável                                    | Fixar versões exatas na Etapa 0.                                                                                                                                                 |
| Vite                     | 8.x (bundler Rolldown)                                                             | Estável desde mar/2026                     | Confirmar plugins (`@vitejs/plugin-react`, `@tailwindcss/vite`) na Etapa 0.                                                                                                      |
| Tailwind CSS             | 4.x + `@tailwindcss/vite`                                                          | Estável                                    | shadcn/ui suporta Tailwind 4 + React 19. Usar `tw-animate-css` (o `tailwindcss-animate` foi descontinuado).                                                                      |
| shadcn/ui + Lucide React | CLI atual / última                                                                 | Estável                                    | Componentes copiados para o repo.                                                                                                                                                |
| Rust                     | stable, edition 2024 + Tokio 1.x                                                   | Estável                                    |                                                                                                                                                                                  |
| windows-rs               | crate `windows`, última versão                                                     | Estável                                    | Features: `Win32_Graphics_Dxgi`, `Win32_Graphics_Direct3D11`, `Win32_Media_MediaFoundation`, `Win32_UI_Input_KeyboardAndMouse`, `Win32_System_Services`.                         |
| **webrtc-rs**            | **0.21.1** (crate `webrtc` + núcleo `rtc` 0.21.1)                                  | Estável (11/10/2026)                       | ✅ D2: arquitetura sans-I/O, independente de runtime (feature `runtime-tokio`). Fica isolada no `kr-transport`.                                                                  |
| Node.js                  | 26 LTS                                                                             | LTS desde 28/10/2026, suporte até abr/2029 | Node 24 entrou em manutenção.                                                                                                                                                    |
| Fastify                  | 5.x                                                                                | Estável                                    | Exige Node 20+. Validar Node 26 no CI.                                                                                                                                           |
| WebSocket                | `@fastify/websocket` (ws)                                                          | Estável                                    |                                                                                                                                                                                  |
| **Prisma**               | **8** (`prisma` CLI 8.0.0-rc.33 / `@prisma/orm-postgres` 8.0.0-rc.23)              | ⚠️ Ainda RC                                | ✅ D3. Exige Node 22.18+ ou 24.11+ (Node 26 ok) e TS 5.9+. API nova (`db.orm.public.Model...`), sem `@prisma/client`. Fixar versão exata e aplicar a "upgrade recipe" a cada RC. |
| PostgreSQL               | Neon                                                                               | Estável                                    | Conexão direta (o pooler recusa parâmetros enviados pelo Prisma 8).                                                                                                              |
| Hospedagem do backend    | **Vercel Functions** (Fluid compute) com WebSocket nativo                          | GA em 20/10/2026                           | ✅ D4. Conexões caem ao atingir a duração máxima da função (padrão: 5 min). Isso exige reconexão automática e estado compartilhado (ver 2.7).                                    |
| Estado compartilhado     | Redis serverless (Upstash via Vercel Marketplace, plano grátis)                    | Estável                                    | Presença + pub/sub entre instâncias da Vercel.                                                                                                                                   |
| TURN/STUN                | **Cloudflare Realtime TURN** (substitui o coturn no MVP)                           | Estável                                    | 1.000 GB/mês grátis, depois US$ 0,05/GB. A Vercel não roda coturn (precisa de UDP e IP fixo). O coturn ≥ 4.18 numa VPS fica como opção futura.                                   |
| pnpm                     | 10.x                                                                               | Estável                                    | Workspaces + `catalog:` para versões compartilhadas.                                                                                                                             |
| CI/CD                    | GitHub Actions `windows-latest` + `tauri-apps/tauri-action`                        | Estável                                    | Gera NSIS/MSI.                                                                                                                                                                   |
| Assinatura de código     | **MVP: sem assinatura** → depois **SignPath Foundation** (grátis para open source) | ✅ D5                                      | Exige repo público, licença OSI, MFA, e o publisher exibido é "SignPath Foundation".                                                                                             |

### Pontos de compatibilidade que moldam a arquitetura

1. **O IPC do Tauri não serve para transportar vídeo.** Enviar frames de 1080p60 do Rust para o WebView via IPC é inviável (cópia + serialização). Isso define como o lado _viewer_ exibe o vídeo (D1).
2. **O WebView2 já tem WebRTC nativo** (Chromium) com decodificação H.264 por hardware e `<video>` de baixa latência (`jitterBufferTarget` próximo de 0).
3. **webrtc-rs ↔ Chromium interoperam em H.264** se usarmos `packetization-mode=1` e o perfil Constrained Baseline (`42e01f`), sem B-frames.
4. **DXGI Desktop Duplication** não captura a _secure desktop_ (UAC, tela de bloqueio) a menos que rode como SYSTEM nessa desktop, e falha em alguns notebooks híbridos ou VMs. Fallback: Windows.Graphics.Capture.
5. **SendInput** é bloqueado pelo UIPI para janelas elevadas (admin). Ctrl+Alt+Del exige `SendSAS` a partir de um serviço. Ambos ficam para a Etapa 8 (serviço Windows).
6. **Atalhos de sistema no viewer** (Win, Alt+Tab) não chegam ao JavaScript; é preciso um hook nativo `WH_KEYBOARD_LL` em Rust enquanto a janela de sessão estiver em foco.
7. **Neon tem cold start.** Presença online e roteamento de sinalização ficam no Redis; o banco guarda só dados duráveis.
8. **Sinalização na Vercel:** o WebSocket funciona, mas cada conexão vive no máximo a duração da função, e clientes diferentes podem cair em instâncias diferentes. Por isso: (a) o cliente Rust reconecta de forma transparente (antes do limite, com jitter); (b) presença e roteamento ficam no Redis com pub/sub, nunca em memória; (c) a sessão WebRTC **não depende** do WebSocket depois de estabelecida, então uma reconexão da sinalização não derruba a sessão remota.
9. **Antivírus e SmartScreen marcam ferramentas de acesso remoto não assinadas.** No MVP (testes nos seus PCs) basta "Mais informações → Executar assim mesmo". A assinatura (SignPath) entra antes de distribuir para terceiros.

---

## 2. Arquitetura proposta

### 2.1 Visão geral

```
┌────────────────────────── KR Desk (Tauri 2) ──────────────────────────┐
│  UI React (WebView2)                                                   │
│   • Tela inicial (meu ID, conectar)   • Diálogo de autorização         │
│   • Janela de sessão (viewer): RTCPeerConnection nativo + <video>      │
│        │ comandos/eventos tipados (tauri-specta)                       │
│  ──────┼────────────────────────────────────────────────────────────── │
│  Núcleo Rust (Tokio)                                                   │
│   SessionManager ── SignalingClient (WSS) ── Identity (Ed25519)        │
│        │                                                               │
│   Pipeline HOST:  DXGI capture → D3D11 BGRA→NV12 → MF H.264 (HW)       │
│                   → webrtc-rs video track ──────────────┐              │
│   Input HOST:     DataChannel → InputInjector → SendInput              │
│   Hook VIEWER:    WH_KEYBOARD_LL → DataChannel                         │
└──────────────────────────────────────────────────────────┼─────────────┘
                                                           │ SRTP/DTLS (P2P)
        ┌──────────── WSS (sinalização) ─────────────┐     │ ou via TURN
        ▼                                            ▼     ▼
┌──────── Backend (Vercel Functions) ────────┐  ┌─ Cloudflare Realtime ─┐
│ Fastify 5 (WebSocket nativo, Fluid)         │  │ STUN + TURN (anycast) │
│  • registro/login por chave Ed25519         │  │ UDP/TCP/TLS :443      │
│  • relay offer/answer/ICE via pub/sub       │  │ credenciais efêmeras  │
│  • gera credenciais TURN (API Cloudflare)   │  │ geradas pelo backend  │
│  • rate limit / auditoria                   │  └───────────────────────┘
│        │                    │               │
│  Redis (Upstash)       Prisma 8 → Neon      │
│  presença + pub/sub    dados duráveis       │
└─────────────────────────────────────────────┘
```

### 2.2 Decisão central: como o viewer exibe o vídeo (D1)

- **Opção A — híbrida (recomendada para o MVP).** O host envia via **webrtc-rs** (necessário, porque captura e codificação são nativas). O viewer recebe com o **WebRTC nativo do WebView2** e renderiza num `<video>`. Vantagens: decodificação por hardware pronta, nenhum frame passa pelo IPC, muito menos código. Desvantagem: duas pilhas WebRTC para depurar.
- **Opção B — tudo em Rust.** O viewer decodifica com Media Foundation e renderiza numa swapchain D3D11 sobreposta à janela Tauri. Latência mínima e controle total, mas bem mais trabalho (janela nativa filha, sincronização com a UI, redimensionamento, DPI). Pode virar uma evolução futura, porque a arquitetura isola o "renderer".

### 2.3 Estrutura do monorepo

```
kr-desk/
├─ apps/
│  ├─ desktop/            # Tauri 2
│  │  ├─ src/             # React + TS + Tailwind 4 + shadcn/ui
│  │  └─ src-tauri/       # binário Tauri (fino; delega aos crates)
│  └─ server/             # Fastify 5 + Prisma 8 (deploy na Vercel)
├─ crates/
│  ├─ kr-proto/           # mensagens de sinalização/DataChannel (serde) + export TS
│  ├─ kr-core/            # SessionManager, máquina de estados, traits (Capturer, Encoder, Injector, Transport)
│  ├─ kr-identity/        # chaves Ed25519, armazenamento (DPAPI), assinatura
│  ├─ kr-signaling/       # cliente WSS (tokio-tungstenite), reconexão
│  ├─ kr-transport/       # adaptador webrtc-rs 0.21 (isola a biblioteca do resto)
│  ├─ kr-capture-win/     # DXGI Desktop Duplication (+ fallback WGC)
│  ├─ kr-codec-win/       # Media Foundation H.264 (HW async MFT + fallback SW)
│  ├─ kr-codec-soft/      # OpenH264, só para testes do pipeline no Linux/CI
│  ├─ kr-input-win/       # SendInput, mapeamento de scancodes, hook LL
│  └─ kr-service-win/     # (Etapa 8) serviço Windows / secure desktop
├─ packages/
│  ├─ protocol/           # tipos TS gerados de kr-proto (ts-rs/specta)
│  └─ config/             # tsconfig, eslint, prettier compartilhados
├─ infra/                # docs de setup (Vercel, Neon, Upstash, Cloudflare TURN)
├─ .github/workflows/     # ci.yml, release-windows.yml, db-migrate.yml
├─ Cargo.toml             # workspace Rust
└─ pnpm-workspace.yaml
```

Princípio: tudo que é específico de Windows fica em crates `*-win` atrás de _traits_ do `kr-core`. Assim a lógica de sessão, o protocolo e a sinalização compilam e são testados em Linux (ambiente remoto do Claude Code), e o CI Windows valida o resto.

### 2.4 Pipeline de baixa latência (host)

- **Captura:** DXGI `AcquireNextFrame` só quando há mudança (dirty rects); textura fica na GPU.
- **Conversão:** BGRA→NV12 via D3D11 Video Processor (sem cópia para a CPU).
- **Codificação:** MFT H.264 de hardware (NVENC, Quick Sync ou AMF expostos via Media Foundation) em modo assíncrono, com `CODECAPI_AVLowLatencyMode`, sem B-frames, GOP longo + keyframe sob demanda (PLI/FIR), bitrate adaptativo (CBR/VBR com teto). Fallback: MFT de software da Microsoft.
- **Cursor:** enviado separadamente pelo DataChannel (forma + posição), desenhado localmente no viewer, o que reduz a latência percebida.
- **Meta de latência:** < 80 ms ponta a ponta em LAN e < 150 ms via internet P2P, medidos no overlay de métricas.
- **Implementado na Etapa 4 (MVP):** captura DXGI copiada para a CPU (staging) → BGRA→NV12 na CPU, em paralelo (≈2 ms em 1080p; redução bilinear para no máximo 1920×1080) → MFT H.264 de hardware assíncrono, com reserva automática no MFT de software → trilha `TrackLocalStaticSample` (H.264 `42e01f`). Keyframe sob demanda via PLI/FIR (interceptor que entrega o RTCP à aplicação) e pela mensagem `keyframe` do DataChannel; keyframe periódico a cada 10 s; com a tela parada, reenvia 1 quadro/s. O viewer usa `jitterBufferTarget = 0`. Conversão na GPU e controle de bitrate adaptativo ficam para a Etapa 7.

### 2.5 Entrada remota

- DataChannel **não confiável/ordenado** para movimento do mouse e **confiável** para cliques, teclas e roda.
- Coordenadas normalizadas (0–65535) → `MOUSEEVENTF_ABSOLUTE | MOUSEEVENTF_VIRTUALDESK` (multi-monitor).
- Teclado por **scancode** (independente de layout, inclusive ABNT2), com tratamento de teclas estendidas.
- Liberação de todas as teclas/botões pressionados ao perder foco ou encerrar a sessão (evita tecla "presa").
- **Implementado na Etapa 5:** DataChannel `input` (sem ordem, `maxRetransmits: 0`) só para movimentos; cliques, roda e teclas no `control`, com a posição junto do clique. No host, `InputController` (`kr-core`) rastreia o que está pressionado e solta tudo ao fechar o canal; `SendInputSink` (`kr-input-win`) mapeia para o monitor principal na área de trabalho virtual. No viewer, hook `WH_KEYBOARD_LL` ativo só com a tela remota aberta e a janela em primeiro plano (scancode e flag estendida direto do Windows); fora do app, mapa `KeyboardEvent.code` → scancode. Cursor: a forma vem da DXGI (`GetFramePointerShape`) pelo canal de controle e vira o `cursor` CSS do `<video>` — sem atraso de vídeo.
- **Seleção de monitor (complemento da Etapa 5):** a captura lista os monitores ligados à área de trabalho (principal primeiro, depois da esquerda para a direita) e envia `monitors` ao viewer; o viewer escolhe com `select_monitor`. A captura reabre a duplicação no monitor escolhido, o encoder é recriado no novo tamanho e a área desse monitor (`SharedArea`) passa a ser o alvo do mouse. "Todas as telas numa imagem só" fica para depois.

### 2.6 Segurança

| Ameaça                                                                | Mitigação                                                                                                                                                                                            |
| --------------------------------------------------------------------- | ---------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Servidor de sinalização malicioso ou comprometido trocando SDP (MITM) | Cada instalação gera um par **Ed25519**; o ID é vinculado à chave pública. O host **assina o fingerprint DTLS + nonce da sessão** e o viewer verifica (TOFU + código curto de verificação opcional). |
| Acesso sem consentimento                                              | Diálogo obrigatório no host (aceitar/recusar, timeout de 30 s, permissões: só visualizar / controlar). Barra de sessão sempre visível com botão de encerrar.                                         |
| Enumeração de IDs / força bruta                                       | Rate limit por IP e por dispositivo; IDs aleatórios de 9 dígitos; backoff exponencial.                                                                                                               |
| Abuso do TURN                                                         | Credenciais efêmeras geradas pelo backend via API da Cloudflare, TTL curto, emitidas só para sessões já autorizadas pelo host. A chave da API fica só no backend.                                    |
| Senha de acesso permanente (futuro)                                   | Armazenada localmente com Argon2id; nunca sai da máquina; verificada por desafio-resposta.                                                                                                           |
| Chave privada exposta                                                 | Protegida com DPAPI (escopo do usuário/máquina).                                                                                                                                                     |
| Transporte                                                            | WebRTC já cifra mídia e dados (DTLS-SRTP); sinalização só por WSS/TLS.                                                                                                                               |
| Atualizações maliciosas                                               | `tauri-plugin-updater` com manifesto assinado + binário com assinatura Authenticode.                                                                                                                 |

### 2.7 Backend e dados

- **Protocolo de sinalização (JSON versionado):** `hello` (desafio) → `auth` (assinatura) → `connect_request` → `incoming_request` → `accept`/`reject` → `sdp_offer`/`sdp_answer` → `ice_candidate` → `bye`.
- **Presença:** chave `presence:{deviceId}` no Redis com TTL renovado por heartbeat (≈ 25 s). Cada instância assina o canal `dev:{deviceId}` dos sockets que atende; mensagens para outro dispositivo são publicadas no canal dele.
- **Reconexão:** o servidor envia `reconnect_soon` antes do limite da função; o cliente abre uma nova conexão antes de fechar a antiga (sem lacuna de presença). Mensagens em trânsito ficam numa fila curta no Redis (TTL de 30 s) e são entregues após a reconexão.
- **Prisma (dados duráveis):** `Device` (id, publicKey, nome, createdAt, lastSeenAt), `SessionLog` (host, viewer, início, fim, tipo de rota: direta/relay, motivo de término), `AbuseEvent`. Contas de usuário ficam fora do MVP.

### 2.8 Fluxo de desenvolvimento (sem ferramentas locais)

- **Ambiente remoto do Claude Code (Linux):** frontend, backend, crates multiplataforma, testes unitários, `cargo check` dos crates comuns.
- **CI (`ci.yml`, a cada PR):** lint + testes em Linux; build e testes dos crates `*-win` em `windows-latest`; artefato de instalador de teste anexado ao PR.
- **Release (`release-windows.yml`, por tag `v*`):** `tauri-action` → NSIS → assinatura → GitHub Release (rascunho) + manifesto do updater.
- **Deploy do servidor:** integração Git da Vercel (preview por PR, produção na `main`) para `apps/server`; migrações Prisma rodam no build de produção (`scripts/vercel-build.mjs`, só na `main`).
- **Segredos:** na Vercel: `DATABASE_URL`, `REDIS_URL`, `CF_TURN_KEY_ID`, `CF_TURN_API_TOKEN`. No GitHub: chave privada do updater e, depois, o token do SignPath.
- **Limite inevitável:** os testes ponta a ponta reais exigem dois PCs Windows. Cada etapa terá um checklist curto para você executar.

---

## 3. Etapas de desenvolvimento

Cada etapa = branch + PR com CI verde + critério de aceite verificável. Tamanho relativo: P / M / G.

| #   | Etapa                               | Entregável                                                                                                                                                                                                            | Critério de aceite                                                                                                                        | Tam. |
| --- | ----------------------------------- | --------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | ----------------------------------------------------------------------------------------------------------------------------------------- | ---- |
| 0   | **Fundação**                        | Monorepo (pnpm + Cargo), Tauri "hello", Fastify "health", lint/format, `ci.yml` e `release-windows.yml`                                                                                                               | Um push gera um instalador NSIS (não assinado) baixável pelo Actions; versões fixadas e documentadas                                      | P    |
| 1   | **Backend de sinalização**          | Fastify na Vercel com WebSocket, registro por chave Ed25519, alocação de ID, presença/pub-sub no Redis, relay de SDP/ICE, reconexão antes do limite da função, schema Prisma 8 + Neon, credenciais TURN da Cloudflare | Testes de integração passando (incluindo dois clientes em instâncias diferentes e queda forçada do socket); deploy de preview funcionando | M    |
| 2   | **App desktop: identidade e UI**    | Geração/armazenamento de chaves, tela inicial com meu ID, campo "conectar", diálogo de autorização, bandeja do sistema                                                                                                | Dois PCs veem o ID um do outro; pedido chega e pode ser aceito/recusado                                                                   | M    |
| 3   | **Transporte P2P**                  | `kr-transport` (webrtc-rs) no host + RTCPeerConnection no viewer; DataChannel com ping/RTT                                                                                                                            | Conexão direta em redes diferentes; com política `relay` forçada, passa pelo TURN                                                         | M    |
| 4   | **Captura + vídeo**                 | DXGI → NV12 → MF H.264 HW → track; viewer exibe; overlay (fps, bitrate, RTT, rota)                                                                                                                                    | 1080p a ≥ 30 fps estável; latência medida dentro da meta em LAN                                                                           | G    |
| 5   | **Controle remoto**                 | Mouse (absoluto, multi-monitor), teclado por scancode, roda, hook LL no viewer, liberação de teclas                                                                                                                   | Uso normal de apps não elevados; ABNT2 correto; sem teclas presas                                                                         | M    |
| 6   | **Endurecimento de segurança**      | Assinatura do fingerprint DTLS, TOFU + código de verificação, rate limits, auditoria, permissões de sessão, revisão de ameaças                                                                                        | Teste de MITM simulado falha conforme esperado; checklist de segurança aprovado                                                           | M    |
| 7   | **Qualidade adaptativa e robustez** | Bitrate/resolução adaptativos, reconexão, troca de monitor, clipboard de texto, fallback WGC e encoder SW                                                                                                             | Sessão sobrevive a perda de pacotes de 5% e troca de rede; funciona em VM                                                                 | G    |
| 8   | **Serviço Windows e elevação**      | Serviço SYSTEM, captura de UAC/tela de bloqueio, Ctrl+Alt+Del (SendSAS), acesso não assistido opcional com senha                                                                                                      | Controlar prompts de UAC e a tela de login                                                                                                | G    |
| 9   | **Distribuição**                    | Assinatura via SignPath Foundation, auto-update, canal de releases, página de download                                                                                                                                | Instalador assinado sem alerta do SmartScreen (após reputação) e atualização automática funcionando                                       | M    |

MVP utilizável = Etapas 0–6. Etapas 7–9 levam a nível de produto.

---

## 4. Decisões

| ID  | Decisão                | Resultado                                                                                                      |
| --- | ---------------------- | -------------------------------------------------------------------------------------------------------------- |
| D1  | Renderização no viewer | ✅ **A** — WebRTC nativo do WebView2 + `<video>`                                                               |
| D2  | Versão da webrtc-rs    | ✅ **0.21.1** (estável)                                                                                        |
| D3  | Prisma                 | ✅ **8** (RC; versão fixada, upgrade controlado)                                                               |
| D4  | Hospedagem             | ✅ **Vercel** (backend) + **Cloudflare Realtime TURN** (relay) + Upstash Redis — tudo nos planos grátis no MVP |
| D5  | Assinatura             | ✅ **Sem assinatura no MVP**; SignPath Foundation (grátis) antes de distribuir                                 |
| D6  | Repositório            | ✅ **Público**, AGPL-3.0, `kryvon-io/kr-desk`                                                                  |
| D7  | Nome                   | ✅ **KR Desk**, `com.krdesk.app`                                                                               |

### Infraestrutura provisionada (Etapa 1)

- **Vercel:** projeto `kr-desk-server` (Root `apps/server`, Fluid, região `gru1`, Node 24.x — o Node 26 ainda não é oferecido; o servidor usa `temporal-polyfill` porque o Prisma 8 precisa de `Temporal`). Proteção de acesso só nos previews.
- **Neon:** projeto `kr-desk` (São Paulo, Postgres 17). Variáveis `DATABASE_URL` (pooler) e `DATABASE_URL_UNPOOLED` na Vercel. O servidor usa a **conexão direta**: o pooler da Neon recusa parâmetros de inicialização que o Prisma 8 envia (`lc_monetary`).
- **Upstash Redis:** `kr-desk-redis` (Free, `gru1`), variável `REDIS_URL` na Vercel.
- **TURN:** adiado. A Cloudflare exige cartão mesmo no plano grátis; o servidor usa só STUN (`stun.cloudflare.com`) até `CF_TURN_KEY_ID`/`CF_TURN_API_TOKEN` serem configurados — sem mudança de código.
- **Migrações:** aplicadas automaticamente apenas em deploys de produção (`apps/server/scripts/vercel-build.mjs`).

### Custos do MVP (estimados)

- Vercel Hobby, Neon Free, Upstash Free, Cloudflare TURN (1.000 GB/mês), GitHub Actions (repo público) → **R$ 0**.
- Observação: o plano Hobby da Vercel é para uso não comercial. Se o KR Desk virar produto pago, é preciso migrar para o Pro.
