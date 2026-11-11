# KR Desk — Validação de stack, arquitetura e plano de etapas

> Status (11/11/2026): Etapas 0–7 concluídas · Etapa 8, parte 1 (serviço: UAC, tela de bloqueio, Ctrl+Alt+Del) em PR · parte 2 (acesso não assistido com senha) a seguir. Revisão de ameaças: `docs/SECURITY-REVIEW.md`. Repositório público `kryvon-io/kr-desk` (AGPL-3.0).

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
5. **SendInput** é bloqueado pelo UIPI para janelas elevadas (admin). Ctrl+Alt+Del exige `SendSAS` a partir de um serviço. Ambos resolvidos na Etapa 8 (serviço Windows, ver "Serviço Windows" em 2.5).
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
- **Implementado na Etapa 4 (MVP):** captura DXGI copiada para a CPU (staging) → BGRA→NV12 na CPU, em paralelo (≈2 ms em 1080p; redução bilinear para no máximo 1920×1080) → MFT H.264 de hardware assíncrono, com reserva automática no MFT de software → trilha `TrackLocalStaticSample` (H.264 `42e01f`). Keyframe sob demanda via PLI/FIR (interceptor que entrega o RTCP à aplicação) e pela mensagem `keyframe` do DataChannel; keyframe periódico a cada 10 s; com a tela parada, reenvia 1 quadro/s. O viewer usa `jitterBufferTarget = 0`. Conversão na GPU fica para depois.
- **Qualidade adaptativa (Etapa 7, parte 1):** o viewer envia `rx_report` a cada 1 s (pacotes recebidos/perdidos no intervalo + RTT). No host, `QualityController` (AIMD): perda > 5% ou RTT acima de 2× a referência + 40 ms → taxa × 0,7; perda > 2% → × 0,9; 3 relatórios limpos → +8% da taxa máxima. Faixa de 0,4 a 8 Mbps; abaixo de 2,5 Mbps cai para 720p e abaixo de 1 Mbps para 540p (com histerese de 30% para subir). O MFT recebe a nova taxa em tempo real (`CODECAPI_AVEncCommonMeanBitRate`); se não aceitar, o encoder é recriado.
- **Robustez (Etapa 7, parte 2):** se a conexão cair, o viewer reinicia o ICE (nova oferta com `iceRestart`) depois de 2 s em `disconnected` (na hora em `failed`), repetindo a cada 5 s por até 3 min (e na hora em que a sinalização volta); o host aguarda esse mesmo prazo sem derrubar vídeo nem sessão e segura os candidatos novos até mandar a resposta. Os dois lados reabrem a conexão com a sinalização ao detectar a queda, e o cliente de sinalização descarta conexões "zumbis" (nada do servidor em 25 s, com ping a cada 10 s). Captura: se a DXGI não abrir, `WgcCapture` (Windows.Graphics.Capture, cursor desenhado na imagem); `KR_DESK_CAPTURE=wgc` força esse caminho.

### 2.5 Entrada remota

- DataChannel **não confiável/ordenado** para movimento do mouse e **confiável** para cliques, teclas e roda.
- Coordenadas normalizadas (0–65535) → `MOUSEEVENTF_ABSOLUTE | MOUSEEVENTF_VIRTUALDESK` (multi-monitor).
- Teclado por **scancode** (independente de layout, inclusive ABNT2), com tratamento de teclas estendidas.
- Liberação de todas as teclas/botões pressionados ao perder foco ou encerrar a sessão (evita tecla "presa").
- **Implementado na Etapa 5:** DataChannel `input` (sem ordem, `maxRetransmits: 0`) só para movimentos; cliques, roda e teclas no `control`, com a posição junto do clique. No host, `InputController` (`kr-core`) rastreia o que está pressionado e solta tudo ao fechar o canal; `SendInputSink` (`kr-input-win`) mapeia para o monitor principal na área de trabalho virtual. No viewer, hook `WH_KEYBOARD_LL` ativo só com a tela remota aberta e a janela em primeiro plano (scancode e flag estendida direto do Windows); fora do app, mapa `KeyboardEvent.code` → scancode. Cursor: a forma vem da DXGI (`GetFramePointerShape`) pelo canal de controle e vira o `cursor` CSS do `<video>` — sem atraso de vídeo.
- **Seleção de monitor (complemento da Etapa 5):** a captura lista os monitores ligados à área de trabalho (principal primeiro, depois da esquerda para a direita) e envia `monitors` ao viewer; o viewer escolhe com `select_monitor`. A captura reabre a duplicação no monitor escolhido, o encoder é recriado no novo tamanho e a área desse monitor (`SharedArea`) passa a ser o alvo do mouse. "Todas as telas numa imagem só" fica para depois.

**Serviço Windows (Etapa 8, parte 1):** o instalador pergunta o modo. "Só para mim" (padrão) instala sem administrador e sem serviço, como antes (útil em PCs corporativos com restrições). "Para todos os usuários" instala em Arquivos de Programas (exige administrador) e registra o serviço `KRDeskService` (LocalSystem, início automático, reinício em falha), que roda o mesmo `kr-desk.exe` com `--service`. O executável se recusa a instalar o serviço fora de Arquivos de Programas.

- O serviço mantém um **agente** (`kr-desk.exe --agent`) como SYSTEM na sessão do console (`CreateProcessAsUser` com o token do serviço e `TokenSessionId`), recriando-o se a sessão mudar ou ele cair.
- O agente segue a área de trabalho ativa (`OpenInputDesktop` + `SetThreadDesktop`), por isso captura e injeta entrada também no UAC, na tela de bloqueio e em janelas de administrador.
- App ↔ agente: named pipe `\\.\pipe\kr-desk-agent-<sessão>` com protocolo binário próprio (`kr_core::agent`). Uma conexão de captura (pedido → eventos de cursor/monitores + quadro BGRA) e uma de entrada (comandos sem resposta). A codificação, a qualidade adaptativa e a rede continuam no app.
- Sem o serviço (ou com `KR_DESK_CAPTURE=local`), o app captura e injeta por conta própria, como antes.
- Ctrl+Alt+Del: botão na barra da sessão → mensagem `ctrl_alt_del` → agente escreve `sas` na saída padrão → o serviço chama `SendSAS` (com a política `SoftwareSASGeneration` liberada para serviços).
- Proteções: o pipe só aceita usuários interativos (sem direito de criar instâncias) e conexões locais. Os dois lados conferem que o outro é o mesmo executável instalado, na mesma sessão. O agente sai quando a entrada padrão (segurada pelo serviço) fecha. Logs do serviço e do agente ficam em `logs\` na pasta de instalação, onde só administradores escrevem.
- Parte 2 (a seguir): acesso não assistido com senha, inclusive na tela de login sem ninguém conectado.

### 2.6 Segurança

| Ameaça                                                                | Mitigação                                                                                                                                                                                                                                                                                                |
| --------------------------------------------------------------------- | -------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Servidor de sinalização malicioso ou comprometido trocando SDP (MITM) | Cada instalação gera um par **Ed25519**; o ID é vinculado à chave pública. **Os dois lados** assinam os fingerprints DTLS que veem (`kr-desk/session/v1` + ID do pedido) e verificam o do outro antes de vídeo/entrada; TOFU por ID (`known-peers.json`) + código de verificação de 6 dígitos (Etapa 6). |
| Acesso sem consentimento                                              | Diálogo obrigatório no host (aceitar/recusar, timeout de 30 s, permissões: só visualizar / controlar). Barra de sessão sempre visível com botão de encerrar.                                                                                                                                             |
| Enumeração de IDs / força bruta                                       | Rate limit por IP e por dispositivo; IDs aleatórios de 9 dígitos; backoff exponencial.                                                                                                                                                                                                                   |
| Abuso do TURN                                                         | Credenciais efêmeras geradas pelo backend via API da Cloudflare, TTL curto, emitidas só para sessões já autorizadas pelo host. A chave da API fica só no backend.                                                                                                                                        |
| Senha de acesso permanente (futuro)                                   | Armazenada localmente com Argon2id; nunca sai da máquina; verificada por desafio-resposta.                                                                                                                                                                                                               |
| Chave privada exposta                                                 | Protegida com DPAPI (escopo do usuário/máquina).                                                                                                                                                                                                                                                         |
| Transporte                                                            | WebRTC já cifra mídia e dados (DTLS-SRTP); sinalização só por WSS/TLS.                                                                                                                                                                                                                                   |
| Atualizações maliciosas                                               | `tauri-plugin-updater` com manifesto assinado + binário com assinatura Authenticode.                                                                                                                                                                                                                     |

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
| 7   | **Qualidade adaptativa e robustez** | Parte 1: bitrate/resolução adaptativos e clipboard de texto (troca de monitor e encoder SW já entregues). Parte 2: reconexão (ICE restart) e fallback WGC                                                             | Sessão sobrevive a perda de pacotes de 5% e troca de rede; funciona em VM                                                                 | G    |
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
