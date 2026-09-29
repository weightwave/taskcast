import { test } from 'node:test'
import assert from 'node:assert/strict'
import { mkdtemp, writeFile, readFile, rm, chmod } from 'node:fs/promises'
import { spawnSync } from 'node:child_process'
import { tmpdir } from 'node:os'
import { join, delimiter } from 'node:path'
import { fileURLToPath } from 'node:url'

for (const fail of ['', 'core']) {
  test(`coverage runner executes all package configs and propagates ${fail || 'no'} failure`, async () => {
    const dir = await mkdtemp(join(tmpdir(), 'coverage-runner-test-'))
    const log = join(dir, 'calls.jsonl')
    try {
      const stub = join(dir, 'pnpm')
      await writeFile(stub, '#!/usr/bin/env node\n' + `
        const fs = require('node:fs'); const path = require('node:path');
        fs.appendFileSync(process.env.COVERAGE_RUNNER_LOG, JSON.stringify({ package: path.basename(process.cwd()), args: process.argv.slice(2) }) + '\\n');
        process.exit(path.basename(process.cwd()) === process.env.COVERAGE_RUNNER_FAIL ? 1 : 0);
      `)
      await chmod(stub, 0o755)
      const result = spawnSync(process.execPath, [fileURLToPath(new URL('../test-coverage.mjs', import.meta.url))], {
        env: { ...process.env, PATH: dir + delimiter + process.env.PATH, COVERAGE_RUNNER_LOG: log, COVERAGE_RUNNER_FAIL: fail }, encoding: 'utf8',
      })
      assert.equal(result.status, fail ? 1 : 0, result.stderr)
      const calls = (await readFile(log, 'utf8')).trim().split('\n').map(line => JSON.parse(line))
      for (const name of ['core', 'server', 'postgres', 'redis', 'cli', 'e2e', 'dashboard-web', 'client', 'react', 'server-sdk', 'sqlite', 'sentry']) assert.ok(calls.some(call => call.package === name), name)
      assert.equal(new Set(calls.map(call => call.package)).size, calls.length)
      for (const call of calls) assert.deepEqual(call.args, ['exec', 'vitest', 'run', '--coverage', '--no-file-parallelism'])
    } finally { await rm(dir, { recursive: true, force: true }) }
  })
}
