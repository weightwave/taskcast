# @taskcast/cli

Standalone [Taskcast](https://github.com/weightwave/taskcast) server. Run a fully configured task tracking service with a single command.

## Quick Start

```bash
npx @taskcast/cli
```

The server starts on port `3721` by default.

## Commands

```
Usage: taskcast [command] [options]

Commands:
  start           Start the Taskcast server in foreground (default)
  daemon          Start as a background service (not yet implemented)
  stop            Stop the background service (not yet implemented)
  status          Show server status (not yet implemented)

Options:
  -V, --version   Show version
  -h, --help      Show help
```

### `taskcast start`

Start the server in foreground mode. This is the default command — `taskcast` is equivalent to `taskcast start`.

```
Options:
  -c, --config <path>   Path to config file
  -p, --port <port>     Server port (default: 3721)
  -s, --storage <type>  Storage backend: memory | redis | sqlite (default: memory)
  --db-path <path>      SQLite database file path (default: ./taskcast.db)
```

## Configuration

### Config File

```bash
npx @taskcast/cli start -p 8080 -c taskcast.config.yaml
```

Taskcast searches for config files in the current directory:

`taskcast.config.ts` > `.js` > `.mjs` > `.yaml` / `.yml` > `.json`

### Environment Variables

| Variable | Description | Default |
|----------|-------------|---------|
| `TASKCAST_PORT` | Server port | `3721` |
| `TASKCAST_AUTH_MODE` | `none` \| `jwt` \| `custom` | `none` |
| `TASKCAST_JWT_SECRET` | JWT HMAC secret | -- |
| `TASKCAST_REDIS_URL` | Redis connection URL | -- |
| `TASKCAST_POSTGRES_URL` | PostgreSQL connection URL | -- |
| `TASKCAST_POSTGRES_MAX_CONNECTIONS` | Maximum PostgreSQL pool connections per Taskcast process; positive integer only | `10` |
| `TASKCAST_STORAGE` | `memory` \| `redis` \| `sqlite` | `memory` |
| `TASKCAST_SQLITE_PATH` | SQLite database file path | `./taskcast.db` |
| `TASKCAST_LOG_LEVEL` | Minimum server log level (`debug`, `info`, `warn`, or `error`); invalid values fail startup. HTTP 5xx failures are emitted as structured JSON on stderr. | `info` |

### Storage Resolution

The short-term/broadcast storage priority is `--storage`, `TASKCAST_STORAGE`,
the configured short-term/broadcast provider, a non-empty Redis URL, then
`memory`. When resolution reaches the configured provider (with no
higher-priority `--storage` or `TASKCAST_STORAGE` selection), the configured
short-term and broadcast providers must match or startup is rejected. Explicit
`memory` and `sqlite` do not activate Redis merely because a Redis URL exists.

PostgreSQL long-term storage is separate. When resolved storage is `sqlite`,
PostgreSQL is disabled before its provider or URL is evaluated. Otherwise, a
configured PostgreSQL provider requires a non-empty `TASKCAST_POSTGRES_URL` or
configured long-term-store URL. Without a configured long-term provider, a
non-empty `TASKCAST_POSTGRES_URL` activates PostgreSQL; a different configured
provider does not.

### Dependency Availability and Recovery

The server checks active Redis and PostgreSQL dependencies before binding HTTP.
`/health` is a liveness endpoint and performs no dependency I/O. `/health/ready`
checks active dependencies and returns `503` when one is unavailable;
`/health/detail` reports sanitized dependency state and never credentials.

An operation interrupted by a disconnect can fail and is not automatically
replayed. Later operations can recover through managed reconnect/pool behavior.
Redis PubSub subscriptions are restored before PubSub is reported ready. Typed
dependency-connectivity failures use HTTP `503` with the existing
`{ "error": string }` business envelope. `dependency_state_change` and
throttled `dependency_outage_summary` records are structured JSON written to
stderr; ship those records through your own logging pipeline if needed.

### SQLite Storage

For zero-dependency local development with persistent storage:

```bash
npx @taskcast/cli start --storage sqlite
```

Data is stored in `./taskcast.db` by default. Customize with `--db-path`:

```bash
npx @taskcast/cli start --storage sqlite --db-path /tmp/my-taskcast.db
```

## Part of Taskcast

This is the CLI package. See the [Taskcast monorepo](https://github.com/weightwave/taskcast) for the full project.

## License

[MIT](https://github.com/weightwave/taskcast/blob/main/LICENSE)
