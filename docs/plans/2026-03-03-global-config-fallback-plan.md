# Global Config Fallback Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Add `~/.taskcast/` as a global config fallback and interactively prompt users to create a default config when none is found.

**Architecture:** `loadConfigFile` in `@taskcast/core` gains a second search pass over `~/.taskcast/` and returns a `{ config, source }` tuple. The CLI layer in `@taskcast/cli` handles the interactive prompt and file creation — core stays free of IO/TTY concerns.

**Tech Stack:** TypeScript, Vitest, Node.js `readline` (for interactive prompt), `os.homedir()`.

---

### Task 1: Update `loadConfigFile` return type and add global fallback

**Files:**
- Modify: `packages/core/src/config.ts:83-115`

**Step 1: Write the failing tests**

Add to `packages/core/tests/unit/config.test.ts`:

```typescript
import { mkdirSync, rmSync } from 'fs'
import { homedir } from 'os'

describe('loadConfigFile - return type with source', () => {
  it('returns source "explicit" when a path is given and file exists', async () => {
    const tmpPath = join(tmpdir(), `taskcast-test-${Date.now()}.yaml`)
    writeFileSync(tmpPath, 'port: 9999\n')
    try {
      const result = await loadConfigFile(tmpPath)
      expect(result.config.port).toBe(9999)
      expect(result.source).toBe('explicit')
    } finally {
      unlinkSync(tmpPath)
    }
  })

  it('returns source "explicit" with empty config when explicit path does not exist', async () => {
    const result = await loadConfigFile('/tmp/taskcast-nonexistent-xyz-12345.yaml')
    expect(result.config).toEqual({})
    expect(result.source).toBe('explicit')
  })

  it('returns source "none" when no config files exist anywhere', async () => {
    // Use a temp dir as cwd where no config files exist, and mock homedir
    const result = await loadConfigFile()
    expect(result.source).toBe('none')
    expect(result.config).toEqual({})
  })
})

describe('loadConfigFile - global fallback', () => {
  const globalDir = join(tmpdir(), `taskcast-global-test-${Date.now()}`)
  const globalConfigPath = join(globalDir, 'taskcast.config.yaml')

  beforeEach(() => {
    mkdirSync(globalDir, { recursive: true })
  })

  afterEach(() => {
    rmSync(globalDir, { recursive: true, force: true })
  })

  it('finds config in global directory when local directory has none', async () => {
    writeFileSync(globalConfigPath, 'port: 5555\n')
    const result = await loadConfigFile(undefined, globalDir)
    expect(result.config.port).toBe(5555)
    expect(result.source).toBe('global')
  })

  it('does not search global for ts/js/mjs files', async () => {
    writeFileSync(join(globalDir, 'taskcast.config.js'), 'export default { port: 1234 }')
    const result = await loadConfigFile(undefined, globalDir)
    expect(result.source).toBe('none')
  })
})
```

**Step 2: Run tests to verify they fail**

Run: `cd packages/core && pnpm test -- tests/unit/config.test.ts`
Expected: FAIL — `result.source` is undefined, `loadConfigFile` returns `TaskcastConfig` not `{ config, source }`.

**Step 3: Update the `loadConfigFile` signature and implementation**

In `packages/core/src/config.ts`, replace lines 83-115 with:

```typescript
export interface ConfigLoadResult {
  config: TaskcastConfig
  source: 'explicit' | 'local' | 'global' | 'none'
}

export async function loadConfigFile(
  configPath?: string,
  globalConfigDir?: string,
): Promise<ConfigLoadResult> {
  const { readFileSync, existsSync } = await import('fs')
  const { resolve, extname, join } = await import('path')
  const { homedir } = await import('os')

  // 1. Explicit path
  if (configPath) {
    const fullPath = resolve(configPath)
    if (!existsSync(fullPath)) return { config: {}, source: 'explicit' }

    const ext = extname(fullPath).toLowerCase()
    /* v8 ignore next 4 -- dynamic import of .ts/.js/.mjs config files */
    if (ext === '.ts' || ext === '.js' || ext === '.mjs') {
      const mod = await import(fullPath) as { default?: TaskcastConfig }
      return { config: mod.default ?? {}, source: 'explicit' }
    }

    const content = readFileSync(fullPath, 'utf8')
    const format = ext === '.json' ? 'json' : 'yaml'
    return { config: parseConfig(content, format), source: 'explicit' }
  }

  // 2. Local directory
  const localCandidates = [
    'taskcast.config.ts',
    'taskcast.config.js',
    'taskcast.config.mjs',
    'taskcast.config.yaml',
    'taskcast.config.yml',
    'taskcast.config.json',
  ]

  for (const candidate of localCandidates) {
    const fullPath = resolve(candidate)
    if (!existsSync(fullPath)) continue

    const ext = extname(fullPath).toLowerCase()
    /* v8 ignore next 4 -- dynamic import of .ts/.js/.mjs config files */
    if (ext === '.ts' || ext === '.js' || ext === '.mjs') {
      const mod = await import(fullPath) as { default?: TaskcastConfig }
      return { config: mod.default ?? {}, source: 'local' }
    }

    const content = readFileSync(fullPath, 'utf8')
    const format = ext === '.json' ? 'json' : 'yaml'
    return { config: parseConfig(content, format), source: 'local' }
  }

  // 3. Global directory (~/.taskcast/) — only static formats
  const globalDir = globalConfigDir ?? join(homedir(), '.taskcast')
  const globalCandidates = [
    'taskcast.config.yaml',
    'taskcast.config.yml',
    'taskcast.config.json',
  ]

  for (const candidate of globalCandidates) {
    const fullPath = join(globalDir, candidate)
    if (!existsSync(fullPath)) continue

    const content = readFileSync(fullPath, 'utf8')
    const ext = extname(fullPath).toLowerCase()
    const format = ext === '.json' ? 'json' : 'yaml'
    return { config: parseConfig(content, format), source: 'global' }
  }

  return { config: {}, source: 'none' }
}
```

**Step 4: Run tests to verify they pass**

Run: `cd packages/core && pnpm test -- tests/unit/config.test.ts`
Expected: PASS

**Step 5: Commit**

```bash
git add packages/core/src/config.ts packages/core/tests/unit/config.test.ts
git commit -m "feat(core): add global config fallback and source info to loadConfigFile"
```

---

### Task 2: Fix existing tests for new return type

The existing `loadConfigFile` tests assert on the old bare `TaskcastConfig` return. They need updating to destructure `{ config }`.

**Files:**
- Modify: `packages/core/tests/unit/config.test.ts:80-116`

**Step 1: Update existing tests**

Replace the existing `describe('loadConfigFile', ...)` block (lines 80-116) with:

```typescript
describe('loadConfigFile', () => {
  it('loads a YAML config file from a given path', async () => {
    const tmpPath = join(tmpdir(), `taskcast-test-${Date.now()}.yaml`)
    writeFileSync(tmpPath, 'port: 9999\n')
    try {
      const { config } = await loadConfigFile(tmpPath)
      expect(config.port).toBe(9999)
    } finally {
      unlinkSync(tmpPath)
    }
  })

  it('loads a JSON config file from a given path', async () => {
    const tmpPath = join(tmpdir(), `taskcast-test-${Date.now()}.json`)
    writeFileSync(tmpPath, JSON.stringify({ port: 7777, logLevel: 'debug' }))
    try {
      const { config } = await loadConfigFile(tmpPath)
      expect(config.port).toBe(7777)
      expect(config.logLevel).toBe('debug')
    } finally {
      unlinkSync(tmpPath)
    }
  })

  it('returns empty config for a nonexistent explicit path', async () => {
    const { config } = await loadConfigFile('/tmp/taskcast-nonexistent-xyz-12345.yaml')
    expect(config).toEqual({})
  })

  it('returns a defined result when no default config files exist', async () => {
    const result = await loadConfigFile()
    expect(result.config).toBeDefined()
  })
})
```

**Step 2: Run all core tests**

Run: `cd packages/core && pnpm test`
Expected: PASS

**Step 3: Commit**

```bash
git add packages/core/tests/unit/config.test.ts
git commit -m "test(core): update loadConfigFile tests for new return type"
```

---

### Task 3: Update CLI to destructure the new return type

**Files:**
- Modify: `packages/cli/src/index.ts:29`

**Step 1: Update the CLI call site**

Change line 29 from:

```typescript
    const fileConfig = await loadConfigFile(options.config)
```

to:

```typescript
    const { config: fileConfig } = await loadConfigFile(options.config)
```

**Step 2: Build to verify**

Run: `pnpm build`
Expected: PASS — no type errors.

**Step 3: Commit**

```bash
git add packages/cli/src/index.ts
git commit -m "refactor(cli): destructure new loadConfigFile return type"
```

---

### Task 4: Add interactive config creation prompt to CLI

**Files:**
- Modify: `packages/cli/src/index.ts`

**Step 1: Add the default config template and prompt logic**

Add above the `program` definition (after imports):

```typescript
import { createInterface } from 'readline'
import { mkdirSync, writeFileSync } from 'fs'
import { join } from 'path'
import { homedir } from 'os'

const DEFAULT_CONFIG_YAML = `# Taskcast configuration
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
`

async function promptCreateGlobalConfig(): Promise<boolean> {
  // Skip in non-TTY environments (CI, Docker, piped stdin)
  if (!process.stdin.isTTY) return false

  const globalConfigPath = join(homedir(), '.taskcast', 'taskcast.config.yaml')

  return new Promise((resolve) => {
    const rl = createInterface({ input: process.stdin, output: process.stdout })
    rl.question(
      `[taskcast] No config file found.\n? Create a default config at ${globalConfigPath}? (Y/n) `,
      (answer) => {
        rl.close()
        const trimmed = answer.trim().toLowerCase()
        resolve(trimmed === '' || trimmed === 'y' || trimmed === 'yes')
      },
    )
  })
}

function createDefaultGlobalConfig(): string {
  const globalDir = join(homedir(), '.taskcast')
  const globalConfigPath = join(globalDir, 'taskcast.config.yaml')
  mkdirSync(globalDir, { recursive: true })
  writeFileSync(globalConfigPath, DEFAULT_CONFIG_YAML)
  console.log(`[taskcast] Created default config at ${globalConfigPath}`)
  return globalConfigPath
}
```

**Step 2: Wire it into the start command**

Update the start command action. After the `loadConfigFile` call (line ~29), add the interactive prompt logic:

```typescript
    const { config: fileConfig, source } = await loadConfigFile(options.config)

    if (source === 'none') {
      const shouldCreate = await promptCreateGlobalConfig()
      if (shouldCreate) {
        const createdPath = createDefaultGlobalConfig()
        const { config: newConfig } = await loadConfigFile(createdPath)
        Object.assign(fileConfig, newConfig)
      }
    }
```

Note: `fileConfig` is reassigned via `Object.assign` so the rest of the function continues to work unchanged — `const` is fine because we mutate the object, not rebind the variable. Actually, since `fileConfig` starts as `{}` for `source === 'none'`, assigning into it works.

Wait — `fileConfig` is declared with `const` destructuring. We need to handle this properly. Let's restructure:

```typescript
    let { config: fileConfig, source } = await loadConfigFile(options.config)

    if (source === 'none') {
      const shouldCreate = await promptCreateGlobalConfig()
      if (shouldCreate) {
        const createdPath = createDefaultGlobalConfig()
        const created = await loadConfigFile(createdPath)
        fileConfig = created.config
      }
    }
```

**Step 3: Build to verify**

Run: `pnpm build`
Expected: PASS

**Step 4: Test manually**

Run from a directory with no config file:
```bash
node packages/cli/dist/index.js
```
Expected: interactive prompt appears asking to create config.

**Step 5: Commit**

```bash
git add packages/cli/src/index.ts
git commit -m "feat(cli): add interactive prompt to create global config when none found"
```

---

### Task 5: Export `ConfigLoadResult` type from core

**Files:**
- Verify: `packages/core/src/index.ts:8`

**Step 1: Verify the export**

`packages/core/src/index.ts` already has `export * from './config.js'` — since `ConfigLoadResult` is exported from `config.ts`, it's already available. No change needed.

**Step 2: Build all packages**

Run: `pnpm build`
Expected: PASS — all packages compile cleanly.

**Step 3: Run all tests**

Run: `pnpm test`
Expected: PASS

**Step 4: Commit (if any changes were needed)**

Skip if no changes.

---

### Task 6: Final verification

**Step 1: Run full test suite**

Run: `pnpm test`
Expected: All tests PASS.

**Step 2: Run type check**

Run: `pnpm lint`
Expected: No errors.

**Step 3: Run test coverage**

Run: `pnpm test:coverage`
Expected: Coverage meets threshold.
