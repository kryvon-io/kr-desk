# Infraestrutura, custos e migração para VPS

Auditoria de custos de 15/11/2026. Complementa `docs/ARCHITECTURE.md` (seção de deploy).

## 1. Infraestrutura atual

| Peça                  | Onde                                     | Plano               | Papel                                                          |
| --------------------- | ---------------------------------------- | ------------------- | -------------------------------------------------------------- |
| Sinalização (Fastify) | Vercel Functions, Fluid compute, `gru1`  | Hobby (grátis)      | WebSocket `/ws`, registro/ID, presença, relay de SDP/ICE       |
| Estado compartilhado  | Upstash Redis `kr-desk-redis`, `gru1`    | Free                | Presença e pub/sub entre instâncias da Vercel                  |
| Banco                 | Neon `kr-desk`, São Paulo, Postgres 17   | Free                | Dispositivos (chave pública → ID, nome, versão, último acesso) |
| TURN                  | Cloudflare Realtime TURN                 | 1.000 GB/mês grátis | Relay de mídia quando o P2P direto falha                       |
| Builds/releases       | GitHub Actions (repo público) + Releases | Grátis              | CI, instaladores Windows, `latest.json` do auto-update         |

A mídia (vídeo/entrada) é **P2P WebRTC**: não passa pelo servidor. O servidor só
participa da autenticação e da troca de SDP/ICE; o TURN só entra quando a rede
bloqueia o P2P. Por isso o servidor é leve em CPU e tráfego, e o custo vem quase
todo de **manter WebSockets abertos** o tempo todo (cada PC fica conectado para
poder ser chamado).

## 2. Problemas de consumo encontrados

1. **Memória provisionada da Vercel (o principal).** No Fluid compute a Vercel
   cobra a memória da instância (fixa em 2 GB no Hobby) **durante toda a vida da
   instância**, inclusive esperando I/O. Com um PC conectado sempre há uma
   instância viva: 2 GB × 720 h ≈ **1.440 GB-h/mês**, contra **360 GB-h** no
   Hobby. No Hobby não há cobrança extra, mas a conta pode ser **pausada** ao
   estourar o limite (sinalização fora do ar). No Pro: US$ 20/mês + US$ 0,0106
   por GB-h excedente. Além disso, o Hobby é só para uso não comercial.
2. **Rotação forçada a cada 270 s.** A duração máxima da função (300 s) obriga
   o cliente a reconectar o tempo todo: cada rotação refaz handshake TLS,
   autenticação, uma consulta e um `UPDATE` no Neon e 3 comandos no Redis.
3. **Comandos do Upstash.** A presença era regravada a **cada ping (10 s)**:
   ≈ 260 mil comandos/mês por conexão, mais as rotações. O plano grátis tem 500
   mil/mês, ou seja, menos de 2 PCs sempre ligados (no modo serviço, cada PC
   mantém 2 conexões: serviço + app).
4. **Neon acordado.** O `touch` (`UPDATE` de último acesso) rodava em toda
   autenticação, ou seja, a cada 270 s por conexão: o compute do Neon nunca
   suspendia e as horas de compute do plano grátis eram consumidas à toa.
5. **Logs.** O health check registrava cada requisição.

## 3. Modificações feitas (PR "infra: VPS + Coolify")

Servidor (`apps/server`), compatível com a Vercel atual:

- Presença no Redis regravada no máximo a cada **25 s** (TTL continua 60 s;
  `PRESENCE_REFRESH_MS` ajusta). Comandos por conexão: −60%.
- `touch` no banco só se o nome/versão mudou ou se o último registro tem mais
  de **6 h**. Na prática, some o `UPDATE` das rotações e o Neon pode suspender.
- `SIGNALING_SINGLE_INSTANCE=1`: num processo único (VPS), o roteamento fica em
  memória e o **Redis deixa de ser necessário**. Se `REDIS_URL` estiver
  definida, ela continua valendo (usado na transição, ver §5).
- `TRUST_PROXY` configurável: na VPS confia só no(s) proxy(s) da frente, para
  o cliente não forjar o IP usado no limite de tentativas.
- Encerramento gracioso (SIGTERM/SIGINT) para deploys sem conexões penduradas;
  `/health` sem log por requisição.
- Build de produção (`pnpm --filter @kr-desk/server build` → `dist/`) e
  `Dockerfile` multi-stage (Node 24, usuário sem privilégios, health check).

Cliente Rust (`crates/kr-signaling`):

- `KR_DESK_SIGNALING_URL`/`DEFAULT_URL` aceitam **lista separada por vírgula**.
  Se o servidor atual falha, o cliente tenta o próximo na hora (failover); só
  depois de esgotar a lista aplica o backoff. Permite trocar de hospedagem sem
  deixar PCs offline e manter a Vercel como reserva.

Entrega:

- `.github/workflows/server-image.yml`: em PR só compila e testa a imagem; na
  `main` publica `ghcr.io/kryvon-io/kr-desk-server` (amd64 + arm64) e, se os
  segredos existirem, aciona o deploy do Coolify.
- `deploy/coolify/docker-compose.yml`: recurso pronto para o Coolify.

## 4. Comparação e recomendação

| Opção                                    | Custo/mês                           | Observações                                                                                               |
| ---------------------------------------- | ----------------------------------- | --------------------------------------------------------------------------------------------------------- |
| Vercel Hobby (hoje)                      | R$ 0                                | Estoura 360 GB-h com PCs sempre conectados → risco de pausa; uso não comercial; rotação a cada 270 s.     |
| Vercel Pro                               | US$ 20 + excedente (≈ US$ 20–32)    | Resolve o limite, mas mantém rotação, Redis e o custo fixo mais alto.                                     |
| **Hetzner CX23** + Coolify (recomendado) | **≈ € 5,99** (€ 5,49 + IPv4 € 0,50) | 2 vCPU/4 GB/40 GB, 20 TB de tráfego. Conexões de 6 h, sem Redis. Só existe na UE (ver latência abaixo).   |
| Hetzner CAX11 (ARM) + Coolify            | ≈ € 6,49                            | Mesma coisa em ARM; a imagem já é publicada para arm64.                                                   |
| Hetzner CPX22                            | ≈ € 19,99                           | Só se CX/CAX estiverem sem estoque; aí o ganho sobre a Vercel Pro some — considere outro provedor de VPS. |

Valores de nov/2026, sem impostos; a Hetzner reajustou preços em 15/06/2026 e a
linha CX/CAX tem ficado **sem estoque** em alguns períodos — confirme no
console antes de contratar. Hetzner nos EUA (CPX11 ≈ € 17,49) não compensa.

**Latência:** a Hetzner barata fica na Alemanha/Finlândia (~200 ms do Brasil).
Isso afeta só a **abertura** da sessão (alguns ida-e-volta de sinalização, +0,5 s
no pior caso). O vídeo segue P2P ou pelo TURN da Cloudflare (anycast, perto do
usuário), então a sessão em si não fica mais lenta. A consulta ao Neon (São
Paulo) acontece só na autenticação, que passa a ocorrer a cada 6 h.

**Mantidos sem mudança:** Neon (grátis; agora suspende quando ocioso),
Cloudflare TURN (grátis até 1 TB), GitHub Actions/Releases (grátis).
**Eliminados após a migração:** Upstash (desnecessário com processo único) e o
risco de pausa da Vercel.

**Custo estimado após migrar:** ≈ **€ 6/mês** (VPS) + domínio, se ainda não
houver um (≈ US$ 10/ano na Cloudflare Registrar). Uma VPS dessas comporta
milhares de WebSockets ociosos; o gargalo futuro seria memória (~50–100 KB por
conexão no Node), não CPU.

**Escala futura:** com mais de uma VPS, basta voltar a definir `REDIS_URL`
(Upstash ou um Redis no próprio Coolify) e remover `SIGNALING_SINGLE_INSTANCE`.

## 5. Migração (sem downtime)

Pré-requisitos (exigem autorização/ação do dono do projeto): VPS contratada,
domínio na Cloudflare, Coolify instalado.

1. **VPS + Coolify.** Criar a VPS (Ubuntu 24.04, chave SSH, firewall da Hetzner
   liberando só 22/80/443). Instalar o Coolify pelo script oficial.
2. **DNS.** Registro `A` (e `AAAA`) `signal.<domínio>` → IP da VPS, **sem proxy**
   (nuvem cinza) e `TRUST_PROXY=1`. Com proxy (nuvem laranja), usar
   `TRUST_PROXY=2` e restringir 80/443 aos IPs da Cloudflare no firewall.
3. **Imagem.** Após o merge, o workflow publica a imagem no GHCR. Deixar o
   pacote `kr-desk-server` público (repo é público) ou criar um token de
   leitura no Coolify.
4. **Recurso no Coolify.** Novo recurso Docker Compose com
   `deploy/coolify/docker-compose.yml`; domínio `https://signal.<domínio>`;
   variáveis: `DATABASE_URL_UNPOOLED`, `CF_TURN_KEY_ID`, `CF_TURN_API_TOKEN` e,
   **nesta fase**, `REDIS_URL` = o mesmo Upstash da Vercel (as duas hospedagens
   se enxergam: um PC na VPS pode ser chamado por outro ainda na Vercel).
   Validar: `curl https://signal.<domínio>/health` e teste local com
   `KR_DESK_SIGNALING_URL=wss://signal.<domínio>/ws`.
5. **Deploy automático (opcional).** No Coolify, gerar o webhook de deploy e um
   token de API; salvar como segredos `COOLIFY_WEBHOOK_URL` e `COOLIFY_TOKEN`
   no GitHub.
6. **Release do app.** Mudar `DEFAULT_URL` para
   `"wss://signal.<domínio>/ws,wss://kr-desk-server.vercel.app/ws"` e publicar
   uma versão; o auto-update leva aos PCs. Quem não atualizou continua na
   Vercel e conversa com os demais via Redis compartilhado.
7. **Observar** alguns dias (logs no Coolify, sessões entre os dois PCs).
8. **Desligar o Redis.** Quando todos estiverem na versão nova, remover
   `REDIS_URL` no Coolify (roteamento em memória). A Vercel fica como reserva:
   o failover dos clientes a usa se a VPS cair — mas sem Redis compartilhado,
   PCs em hospedagens diferentes não se enxergam. Para reserva "quente",
   mantenha o `REDIS_URL` nas duas.
9. **Encerrar a Vercel/Upstash** só depois de validado; até lá não custam nada.
   Antes de encerrar a Vercel, mover as migrações do Prisma (hoje rodam no
   build de produção da Vercel, `scripts/vercel-build.mjs`) para um job manual
   do GitHub Actions ou para o início do contêiner.

## 6. Rollback

- **Servidor:** o app sempre tem a Vercel na lista de URLs. Parar o contêiner
  no Coolify (ou apagar o DNS) faz os clientes caírem na Vercel em segundos.
  Se o Redis já tiver sido removido da VPS, recoloque `REDIS_URL` antes, ou
  pare a VPS de vez para todos irem à Vercel.
- **App:** publicar uma versão com `DEFAULT_URL` só da Vercel (ou reverter o
  commit); o auto-update distribui.
- **Banco:** nenhuma migração de schema foi feita; Vercel e VPS usam o mesmo
  Neon, sem cópia de dados.
- **Mudanças do servidor desta PR:** todas têm padrão compatível com a Vercel;
  reverter o commit volta ao comportamento anterior.

## 7. Segurança

- Segredos só nas variáveis do Coolify, na Vercel e nos GitHub Secrets.
- Contêiner sem root, só a porta 3000 interna; TLS pelo Traefik do Coolify
  (Let's Encrypt).
- O workflow de imagem tem `contents: read`; só o job de publicação (push na
  `main`) recebe `packages: write`. Nada de `pull_request_target`.
- Manter o Ubuntu com `unattended-upgrades` e o Coolify atualizado; acesso SSH
  só por chave.
