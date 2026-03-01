# @taskcast/cli

Standalone [Taskcast](https://github.com/weightwave/taskcast) server. Run a fully configured task tracking service with a single command.

## Quick Start

```bash
npx taskcast
```

The server starts on port `3721` by default.

## Options

```
Usage: taskcast [options] [command]

Commands:
  start           Start the Taskcast server (default)

Options:
  -c, --config    Path to config file
  -p, --port      Server port (default: 3721)
```

## Configuration

### Config File

```bash
npx taskcast -p 8080 -c taskcast.config.yaml
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
| `TASKCAST_LOG_LEVEL` | `debug` \| `info` \| `warn` \| `error` | `info` |

## Part of Taskcast

This is the CLI package. See the [Taskcast monorepo](https://github.com/weightwave/taskcast) for the full project.

## License

[MIT](https://github.com/weightwave/taskcast/blob/main/LICENSE)
