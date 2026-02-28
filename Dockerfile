# ── Stage 1: base ──────────────────────────────────────────────
FROM node:22-alpine AS base
RUN corepack enable && corepack prepare pnpm@10 --activate
WORKDIR /app

# ── Stage 2: build ─────────────────────────────────────────────
FROM base AS build
COPY pnpm-lock.yaml pnpm-workspace.yaml package.json .npmrc ./
COPY packages/core/package.json packages/core/
COPY packages/server/package.json packages/server/
COPY packages/cli/package.json packages/cli/
COPY packages/redis/package.json packages/redis/
COPY packages/postgres/package.json packages/postgres/
COPY packages/sentry/package.json packages/sentry/
COPY packages/client/package.json packages/client/
COPY packages/server-sdk/package.json packages/server-sdk/
COPY packages/react/package.json packages/react/
COPY tsconfig.base.json ./
RUN pnpm install --frozen-lockfile
COPY packages/ packages/
RUN pnpm build

# ── Stage 3: deploy ────────────────────────────────────────────
FROM base AS deploy
COPY --from=build /app /app
RUN pnpm deploy --filter=@taskcast/cli --prod /prod

# ── Stage 4: runtime ───────────────────────────────────────────
FROM node:22-alpine AS runtime
WORKDIR /app
COPY --from=deploy /prod/node_modules ./node_modules
COPY --from=deploy /prod/dist ./dist
COPY --from=deploy /prod/package.json ./

ENV NODE_ENV=production
EXPOSE 3721
CMD ["node", "dist/index.js"]
