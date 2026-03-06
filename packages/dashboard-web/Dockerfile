FROM node:22-alpine AS builder
RUN corepack enable && corepack prepare pnpm@10 --activate
WORKDIR /app
COPY package.json pnpm-lock.yaml pnpm-workspace.yaml .npmrc ./
COPY packages/ packages/
COPY tsconfig.base.json ./
RUN pnpm install --frozen-lockfile && pnpm --filter @taskcast/dashboard-web build

FROM nginx:alpine
RUN apk add --no-cache jq
COPY --from=builder /app/packages/dashboard-web/dist /usr/share/nginx/html
COPY packages/dashboard-web/nginx.conf /etc/nginx/conf.d/default.conf
COPY packages/dashboard-web/docker-entrypoint.sh /docker-entrypoint.sh
ENV TASKCAST_SERVER_URL=http://localhost:3721
EXPOSE 80
ENTRYPOINT ["/docker-entrypoint.sh"]
