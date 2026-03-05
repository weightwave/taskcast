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
| `TASKCAST_STORAGE` | `memory` \| `redis` \| `sqlite` | `memory` |
| `TASKCAST_SQLITE_PATH` | SQLite database file path | `./taskcast.db` |
| `TASKCAST_LOG_LEVEL` | `debug` \| `info` \| `warn` \| `error` | `info` |

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
