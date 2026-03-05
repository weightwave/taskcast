# Global Config Fallback + Interactive Creation

## Problem

When no config file exists in the current directory, `loadConfigFile` silently returns `{}` and the server starts with all defaults. Users get no guidance on where to put a config file or what options are available.

## Design

### Config file search order

`loadConfigFile` searches in this order:

1. **Explicit path** — user passes `-c <path>` (highest priority)
2. **Local directory** — `taskcast.config.{ts,js,mjs,yaml,yml,json}` in CWD
3. **Global directory** — `~/.taskcast/taskcast.config.{yaml,yml,json}` (no ts/js/mjs to avoid executing arbitrary code from a global location)

### Return value change

`loadConfigFile` returns `{ config: TaskcastConfig; source: 'explicit' | 'local' | 'global' | 'none' }` instead of a bare `TaskcastConfig`. This lets the CLI layer decide what to do when no config is found.

### Interactive prompt (CLI only)

When `source === 'none'`, the CLI prints:

```
[taskcast] No config file found.
? Create a default config at ~/.taskcast/taskcast.config.yaml? (Y/n)
```

- **Y / Enter** — creates `~/.taskcast/` directory and writes a default YAML config, then loads it
- **n** — skips, starts with defaults (current behavior)
- **Non-TTY** (CI, Docker, piped stdin) — skips silently

### Default config template

```yaml
# Taskcast configuration
# Docs: https://github.com/weightwave/taskcast

port: 3721

# auth:
#   mode: none  # none | jwt

# adapters:
#   broadcast:
#     provider: memory  # memory | redis
#     # url: redis://localhost:6379
#   shortTerm:
#     provider: memory  # memory | redis
#     # url: redis://localhost:6379
#   longTerm:
#     provider: postgres
#     # url: postgresql://localhost:5432/taskcast
```

### Affected packages

| Package | File | Change |
|---------|------|--------|
| `@taskcast/core` | `config.ts` | Add `~/.taskcast/` fallback search, change return type to include `source` |
| `@taskcast/cli` | `index.ts` | Handle `source === 'none'`, interactive prompt, write default config |
| `@taskcast/core` | `tests/` | Test global fallback search |

### Not changed

- Existing users with config files: behavior unchanged
- `-c` pointing to a missing file: returns `{ config: {}, source: 'explicit' }`, no prompt
- Other packages (server-sdk, client, react): unaffected
