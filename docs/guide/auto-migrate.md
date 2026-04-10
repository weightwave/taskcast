# Auto-Migrate Guide

## Overview

Auto-migrate automatically applies pending PostgreSQL migrations when the Taskcast server starts. This feature eliminates manual database schema synchronization and ensures your database stays up-to-date with the latest schema version.

**When is this useful?**

- **Production deployments** — Automatically sync your database schema on every server restart without manual intervention
- **Development environments** — Seamlessly handle schema migrations as the codebase evolves
- **Docker/Kubernetes** — Eliminate the need for separate migration containers or initialization steps
- **Zero-downtime deployments** — Migrations run before the server accepts traffic

## How It Works

When you start the Taskcast server, the auto-migrate process:

1. Checks if auto-migration is enabled (via `TASKCAST_AUTO_MIGRATE` environment variable)
2. Verifies that PostgreSQL is configured (via `TASKCAST_POSTGRES_URL` or config file)
3. Compares your database schema against embedded migrations
4. Applies any pending migrations in sequence, in order of version
5. Logs the result (migrations applied, database up-to-date, or failure details)

If any migration fails, the server startup is blocked and an error is thrown. This is intentional — migrations are critical, and the server should not start with an inconsistent database.

## Configuration

### Enable Auto-Migration

Set the `TASKCAST_AUTO_MIGRATE` environment variable to enable the feature:

```bash
export TASKCAST_AUTO_MIGRATE=true
npx @taskcast/cli start
```

Recognized truthy values (case-insensitive):
- `true`, `1`, `yes`, `on`

All other values (including empty string, `false`, `0`, `no`) disable auto-migration.

### PostgreSQL URL

Auto-migration requires a PostgreSQL connection URL. Specify it via one of these methods (in order of priority):

1. **Environment variable** (highest priority):
   ```bash
   export TASKCAST_POSTGRES_URL="postgresql://user:password@localhost/taskcast"
   export TASKCAST_AUTO_MIGRATE=true
   npx @taskcast/cli start
   ```

2. **Config file** (via `adapters.longTermStore.url`):
   ```yaml
   # taskcast.config.yaml
   adapters:
     longTermStore:
       url: postgresql://user:password@localhost/taskcast
   ```

3. **CLI flag** (only for manual `taskcast migrate` command):
   ```bash
   npx @taskcast/cli migrate --url "postgresql://user:password@localhost/taskcast"
   ```

## Examples

### Docker / Kubernetes with Auto-Migrate

```dockerfile
FROM node:20-alpine
RUN npm install -g @taskcast/cli
ENV TASKCAST_AUTO_MIGRATE=true
ENV TASKCAST_POSTGRES_URL=postgresql://user:pass@postgres:5432/taskcast
ENV TASKCAST_STORAGE=redis
ENV TASKCAST_REDIS_URL=redis://redis:6379
CMD ["taskcast", "start", "--port", "3721"]
```

When the container starts, auto-migrate automatically runs pending migrations before the server accepts traffic.

### Docker Compose

```yaml
version: '3.8'
services:
  postgres:
    image: postgres:16-alpine
    environment:
      POSTGRES_USER: taskcast
      POSTGRES_PASSWORD: secret
      POSTGRES_DB: taskcast
    volumes:
      - postgres_data:/var/lib/postgresql/data

  redis:
    image: redis:7-alpine

  taskcast:
    image: taskcast:latest
    depends_on:
      - postgres
      - redis
    environment:
      TASKCAST_AUTO_MIGRATE: 'true'
      TASKCAST_POSTGRES_URL: postgresql://taskcast:secret@postgres:5432/taskcast
      TASKCAST_REDIS_URL: redis://redis:6379
    ports:
      - '3721:3721'

volumes:
  postgres_data:
```

### Conditional Auto-Migrate

Auto-migrate is disabled by default. To enable it only in certain environments:

```bash
# Production — auto-migrate enabled
TASKCAST_AUTO_MIGRATE=true TASKCAST_POSTGRES_URL=postgres://... npx @taskcast/cli start

# Development — manual migration via CLI
npx @taskcast/cli migrate --url postgresql://localhost/taskcast_dev
```

### Multi-Host Deployments

**Coordinated startup is not safe with auto-migrate enabled.** If multiple
Taskcast instances start simultaneously against the same database with
`TASKCAST_AUTO_MIGRATE=true`, only one instance can win the race to insert
a given migration row into `_sqlx_migrations`; the others will fail with a
primary-key conflict wrapped as `Auto-migration failed: duplicate key ...`
and exit non-zero.

The Rust CLI benefits from sqlx's built-in advisory locking (migrations run
under a Postgres advisory lock), so concurrent Rust startups serialize
correctly. The Node.js CLI does **not** take an advisory lock and is unsafe
under concurrent startup.

**Recommended patterns:**

```bash
# Option A — run migrations as a pre-deploy step, then start instances with
# auto-migrate disabled (safe for any number of replicas, both CLIs):
TASKCAST_POSTGRES_URL=postgres://... npx @taskcast/cli migrate --yes
TASKCAST_AUTO_MIGRATE=false npx @taskcast/cli start &
TASKCAST_AUTO_MIGRATE=false npx @taskcast/cli start &

# Option B — gate startup through a Kubernetes initContainer or a
# docker-compose "depends_on: condition: service_completed_successfully"
# so only the first pod runs migrations.

# Option C — use the Rust CLI (taskcast-rs), whose advisory-lock protocol
# tolerates concurrent auto-migrate across replicas.
```

## Manual Migration

If you prefer to control migrations manually, use the `taskcast migrate` command:

```bash
# Discover pending migrations and show confirmation
npx @taskcast/cli migrate --url "postgresql://localhost/taskcast"

# Apply migrations without confirmation (CI/CD safe)
npx @taskcast/cli migrate --url "postgresql://localhost/taskcast" --yes
```

### Migration Command Options

```bash
npx @taskcast/cli migrate [options]

Options:
  --url <url>       Postgres URL (highest priority)
  -c, --config <path>  Config file path
  -y, --yes         Skip confirmation prompt
  -h, --help        Show help
```

**URL Resolution Priority:**
1. `--url` flag
2. `TASKCAST_POSTGRES_URL` environment variable
3. `adapters.longTermStore.url` in config file

If no URL is found, the command exits with an error.

### Migration Command Examples

```bash
# Interactive mode — shows pending migrations and asks for confirmation
npx @taskcast/cli migrate --url "postgresql://localhost/taskcast"

# CI/CD mode — apply migrations without prompting
npx @taskcast/cli migrate --url "postgresql://localhost/taskcast" --yes

# Via environment variable
TASKCAST_POSTGRES_URL="postgresql://localhost/taskcast" npx @taskcast/cli migrate --yes

# Via config file
npx @taskcast/cli migrate -c taskcast.config.yaml --yes
```

## Error Handling

### Migration Fails on Startup

If a migration fails during server startup, the server does not start. This is intentional — your database must be in a valid state before accepting requests.

```
[taskcast] Auto-migration failed: constraint violation on public.tasks
```

**What to do:**

1. Check the error message for details (constraint, syntax, permissions, etc.)
2. Investigate your database state (e.g., `psql -U user -d taskcast -c "\d+"`)
3. Fix the underlying issue (schema conflict, insufficient permissions, corrupted data)
4. Restart the server

### PostgreSQL Not Configured

If `TASKCAST_AUTO_MIGRATE=true` but no PostgreSQL URL is found:

```
[taskcast] TASKCAST_AUTO_MIGRATE is set but no Postgres configured — skipping
```

Auto-migration logs this explicit skip message on stderr and the server
continues to start normally. Operators can grep for this line to confirm the
skip was intentional. Set a PostgreSQL URL if you want migrations to run.

### Database Permissions

Migrations require the following PostgreSQL permissions:
- `CREATE TABLE` — to create the `_sqlx_migrations` tracking table
- `CREATE INDEX` — to create database indexes (if any migrations use them)
- `ALTER TABLE` — to modify existing tables
- `SELECT`, `INSERT`, `UPDATE`, `DELETE` — to manage migration records

Ensure your Postgres user has appropriate permissions:

```sql
-- Grant permissions to taskcast user
GRANT CREATE ON DATABASE taskcast TO taskcast;
GRANT USAGE ON SCHEMA public TO taskcast;
GRANT CREATE ON SCHEMA public TO taskcast;
```

### No Pending Migrations

If the database is already up-to-date:

```
[taskcast] Database schema up to date (2 migration(s) already applied)
```

Server continues normally. This is the expected state after the first successful migration run. The count in parentheses is the number of migrations already present in the `_sqlx_migrations` tracking table.

## Disabling Auto-Migrate

To explicitly disable auto-migration:

```bash
export TASKCAST_AUTO_MIGRATE=false
npx @taskcast/cli start
```

Or omit the environment variable entirely — auto-migrate is disabled by default:

```bash
npx @taskcast/cli start  # auto-migrate disabled
```

## Migration Versioning

Migrations are versioned using a numeric prefix in the filename:

```
001_init_schema.sql
002_add_indexes.sql
003_add_webhook_table.sql
```

Each migration is executed exactly once, tracked in the `_sqlx_migrations` table. The version number determines execution order — migrations are always run in ascending order by version.

### Tracking Table

Taskcast uses the `_sqlx_migrations` table to track which migrations have been applied:

```sql
CREATE TABLE _sqlx_migrations (
    version BIGINT PRIMARY KEY,              -- e.g., 1, 2, 3
    description TEXT NOT NULL,               -- e.g., "init schema"
    installed_on TIMESTAMPTZ NOT NULL,       -- when it ran
    success BOOLEAN NOT NULL,                -- whether it succeeded
    checksum BYTEA NOT NULL,                 -- SHA384 of migration SQL
    execution_time BIGINT NOT NULL           -- duration in nanoseconds
)
```

This table is created automatically on first run. You should never edit it manually — Taskcast manages it exclusively.

## Rust Implementation

The Rust CLI (`taskcast-rs`) supports auto-migrate identically to the TypeScript version. All environment variables, configuration, and behavior are synchronized:

- `TASKCAST_AUTO_MIGRATE` works in Rust
- `TASKCAST_POSTGRES_URL` works in Rust
- `taskcast migrate` command available in Rust
- Same migration format, versioning, and tracking table

This means you can upgrade from TypeScript to Rust without changing your deployment configuration.

## Troubleshooting

### "Auto-migration failed: relation already exists"

A table already exists in your database, likely from a previous migration or manual schema creation.

**Solution:**
1. Check if the table is needed — if not, drop it: `DROP TABLE tablename;`
2. Re-run auto-migrate or manually execute the migration

### "Auto-migration failed: permission denied"

Your PostgreSQL user lacks necessary permissions.

**Solution:**
1. Verify the user has `CREATEDB` privilege
2. Grant permissions on the schema: `GRANT CREATE ON SCHEMA public TO user;`
3. Ensure the user owns or has privileges on the database

### "Auto-migration failed: connection refused"

The PostgreSQL server is unreachable.

**Solution:**
1. Check the connection URL: `TASKCAST_POSTGRES_URL`
2. Verify PostgreSQL is running: `psql postgresql://...`
3. Check network connectivity and firewall rules

### Migrations Not Running

If auto-migrate seems to skip:

**Checklist:**
1. Is `TASKCAST_AUTO_MIGRATE` set to a truthy value? (Check: `echo $TASKCAST_AUTO_MIGRATE`)
2. Is `TASKCAST_POSTGRES_URL` configured? (Check: `echo $TASKCAST_POSTGRES_URL`)
3. Check the server logs — Taskcast should log migration status. The banner
   uses a display-formatted URL (host:port/dbname) — credentials are stripped
   so it is safe to include the log line in bug reports or shared dashboards:
   ```
   [taskcast] TASKCAST_AUTO_MIGRATE enabled — running Postgres migrations on host:5432/db
   [taskcast] Applied 3 new migration(s): 001_initial.sql, 002_workers.sql, 003_indexes.sql
   [taskcast] Database schema up to date (5 migration(s) already applied)
   [taskcast] TASKCAST_AUTO_MIGRATE is set but no Postgres configured — skipping
   ```

### "TTY not detected" When Running Migrate

The `taskcast migrate` command requires interactive confirmation unless you use `--yes`:

```bash
# Will fail without TTY:
docker exec taskcast-container taskcast migrate --url postgresql://...

# Use --yes flag instead:
docker exec taskcast-container taskcast migrate --url postgresql://... --yes
```

## Related Documentation

- **Deployment Guide** (`docs/guide/deployment.md`) — Full deployment patterns for embedded and remote modes
- **Configuration** (`docs/guide/`) — Complete configuration reference
- **API Reference** — REST endpoints and migration status
