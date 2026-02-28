# Deployment Configuration Design

**Date:** 2026-02-28
**Status:** Approved

## Goal

Add Dockerfile, docker-compose.yml, Railway, and Fly.io configuration to enable self-hosted deployment of Taskcast as a standalone service.

## Dockerfile — Multi-Stage Build

Four stages to minimize final image size:

| Stage | Base | Purpose |
|-------|------|---------|
| `base` | `node:22-alpine` | Install pnpm via corepack |
| `build` | base | `pnpm install` + `pnpm build` (full monorepo compile) |
| `deploy` | base | `pnpm deploy --prod --filter @taskcast/cli` extracts minimal production deps |
| `runtime` | `node:22-alpine` | Clean image, copy only deploy output + built dist |

Key decisions:
- `pnpm deploy` flattens workspace dependencies into a standalone `node_modules` — no workspace links in production
- Final image contains no devDependencies, no source `.ts` files, no build tooling
- Expose port 3721 (Taskcast default)
- Entrypoint: `node dist/index.js`
- All configuration via environment variables (`TASKCAST_REDIS_URL`, `TASKCAST_POSTGRES_URL`, etc.)

## docker-compose.yml — Multi-Profile

| Profile | Services | Use Case |
|---------|----------|----------|
| *(default)* | taskcast | In-memory mode, quick demo |
| `redis` | taskcast + redis:7-alpine | Broadcast + short-term store |
| `full` | taskcast + redis:7-alpine + postgres:16-alpine | Full production stack |

Usage:
```bash
docker compose up                         # in-memory only
docker compose --profile redis up         # + Redis
docker compose --profile full up          # + Redis + PostgreSQL
```

All services share a single Docker network. Environment variables configured via `.env` file.

## .env.example

Reference file for all supported environment variables with sensible defaults and comments.

## .dockerignore

Exclude `node_modules/`, `dist/`, `.git/`, `coverage/`, `.env`, docs, and test files from build context.

## railway.toml

- Builder: dockerfile
- Health check: `GET /tasks`
- Restart on failure
- Users add Redis/PostgreSQL via Railway addons (auto-injected env vars)

## fly.toml

- Primary region: `nrt` (Tokyo)
- Internal port: 3721, force HTTPS
- HTTP health check on `/tasks`
- Users provision Fly Postgres and Upstash Redis separately

## File List

| File | New/Modified |
|------|-------------|
| `Dockerfile` | New |
| `docker-compose.yml` | New |
| `.dockerignore` | New |
| `.env.example` | New |
| `railway.toml` | New |
| `fly.toml` | New |

No existing code modifications required.
