# Servidor de sinalização do KR Desk para VPS (Hetzner + Coolify).
# Build a partir da RAIZ do repositório (monorepo pnpm):
#   docker build -f apps/server/Dockerfile -t kr-desk-server .

# --- dependências + build ----------------------------------------------------
FROM node:24-slim AS build
RUN npm install -g pnpm@12.18.0
WORKDIR /repo
# Só os manifestos primeiro: a camada de dependências é reaproveitada enquanto
# o lockfile não muda.
COPY package.json pnpm-lock.yaml pnpm-workspace.yaml ./
COPY apps/server/package.json apps/server/
RUN pnpm install --frozen-lockfile --filter @kr-desk/server... --config.engine-strict=false
COPY apps/server apps/server
RUN pnpm --filter @kr-desk/server build

# --- só dependências de produção --------------------------------------------
FROM node:24-slim AS prod-deps
RUN npm install -g pnpm@12.18.0
WORKDIR /repo
COPY package.json pnpm-lock.yaml pnpm-workspace.yaml ./
COPY apps/server/package.json apps/server/
RUN pnpm install --frozen-lockfile --prod --filter @kr-desk/server... --config.engine-strict=false

# --- imagem final ------------------------------------------------------------
FROM node:24-slim
ENV NODE_ENV=production \
    PORT=3000 \
    # Processo único: roteamento em memória, sem Redis (ver docs/INFRA.md).
    SIGNALING_SINGLE_INSTANCE=1 \
    # Sem o limite de 5 min da Vercel: conexões duram horas (menos reconexões,
    # menos consultas ao banco).
    WS_MAX_LIFETIME_MS=21600000 \
    # Só o proxy do Coolify (Traefik) na frente; use 2 com o proxy da Cloudflare.
    TRUST_PROXY=1
WORKDIR /repo
COPY --from=prod-deps /repo/node_modules ./node_modules
COPY --from=prod-deps /repo/apps/server/node_modules ./apps/server/node_modules
COPY --from=build /repo/apps/server/dist ./apps/server/dist
COPY apps/server/package.json ./apps/server/
WORKDIR /repo/apps/server
USER node
EXPOSE 3000
HEALTHCHECK --interval=30s --timeout=5s --start-period=20s --retries=3 \
  CMD node -e "fetch('http://127.0.0.1:'+(process.env.PORT||3000)+'/health').then(r=>process.exit(r.ok?0:1),()=>process.exit(1))"
CMD ["node", "--enable-source-maps", "dist/index.js"]
