FROM node:20-alpine AS builder
WORKDIR /app
COPY package.json pnpm-lock.yaml pnpm-workspace.yaml ./
COPY packages/ packages/
RUN npm install -g pnpm && pnpm install --frozen-lockfile && pnpm --filter @taskcast/dashboard-web build

FROM nginx:alpine
COPY --from=builder /app/packages/dashboard-web/dist /usr/share/nginx/html
COPY packages/dashboard-web/nginx.conf /etc/nginx/templates/default.conf.template
ENV TASKCAST_SERVER_URL=http://localhost:3721
EXPOSE 80
