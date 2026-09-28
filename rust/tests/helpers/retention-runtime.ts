import { spawn, type ChildProcess } from 'node:child_process'
import { once } from 'node:events'
import { createServer } from 'node:net'
import { createRequire } from 'node:module'
import { mkdtemp, writeFile, rm } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join, resolve } from 'node:path'

export const repoRoot = resolve(import.meta.dirname, '../../..')
const require = createRequire(join(repoRoot, 'packages/core/package.json'))
export const { GenericContainer, Wait } = require('testcontainers') as typeof import('../../../packages/core/node_modules/testcontainers')
export const postgres = require('postgres') as typeof import('../../../packages/core/node_modules/postgres').default
export type Runtime = 'node' | 'rust'

async function availablePort(): Promise<number> {
  return new Promise((resolvePort, reject) => {
    const server = createServer()
    server.on('error', reject)
    server.listen(0, '127.0.0.1', () => {
      const address = server.address()
      if (!address || typeof address === 'string') return reject(new Error('No parity port'))
      server.close(() => resolvePort(address.port))
    })
  })
}
export async function until(check: () => Promise<boolean>, timeout = 20_000): Promise<void> {
  const deadline = Date.now() + timeout
  while (Date.now() < deadline) {
    if (await check()) return
    await new Promise(resolveWait => setTimeout(resolveWait, 25))
  }
  throw new Error('Retention parity condition did not become true')
}

export class RetentionRuntime {
  private process?: ChildProcess
  private dir?: string
  private output = ''
  baseUrl = ''
  constructor(readonly runtime: Runtime, readonly redisUrl: string, readonly postgresUrl: string) {}
  async start(enabled: boolean, afterMs = 500): Promise<void> {
    await this.stop()
    this.dir ??= await mkdtemp(join(tmpdir(), 'taskcast-retention-'))
    const config = join(this.dir, 'config.json')
    await writeFile(config, JSON.stringify({
      auth: { mode: 'none' },
      cleanup: { enabled, rules: [{ target: 'events', match: { status: ['completed', 'cancelled'] }, trigger: { afterMs } }, { target: 'events', match: { status: ['failed'] }, trigger: { afterMs: 60_000 } }] },
      storageLifecycle: { ttlSweepIntervalSeconds: 1 },
    }))
    const port = await availablePort()
    this.baseUrl = `http://127.0.0.1:${port}`
    const args = ['start', '--port', String(port), '--config', config]
    this.output = ''
    this.process = this.runtime === 'node'
      ? spawn(process.execPath, [join(repoRoot, 'packages/cli/dist/index.js'), ...args], this.spawnOptions())
      : spawn(process.env['TASKCAST_RUST_BINARY'] ?? join(repoRoot, 'rust/target/debug/taskcast'), args, this.spawnOptions())
    this.process.stdout!.on('data', data => { this.output = (this.output + data).slice(-12_000) })
    this.process.stderr!.on('data', data => { this.output = (this.output + data).slice(-12_000) })
    this.process.on('error', error => { this.output += error.message })
    try {
      await until(async () => {
        if (this.process?.exitCode !== null) throw new Error(`Parity server exited: ${this.output}`)
        try { return (await this.request('/health')).ok } catch { return false }
      })
    } catch (error) { throw new Error(`${String(error)}\n${this.runtime}: ${this.output}`) }
  }
  private spawnOptions() {
    return { cwd: repoRoot, env: { ...process.env, TASKCAST_REDIS_URL: this.redisUrl, TASKCAST_POSTGRES_URL: this.postgresUrl, TASKCAST_REDIS_PREFIX: `retention-${this.runtime}`, TASKCAST_AUTO_MIGRATE: 'true', NO_PROXY: '127.0.0.1,localhost', no_proxy: '127.0.0.1,localhost' }, stdio: ['ignore', 'pipe', 'pipe'] as ['ignore', 'pipe', 'pipe'] }
  }
  request(path: string, body?: unknown, method = 'POST'): Promise<Response> {
    return fetch(this.baseUrl + path, body === undefined ? undefined : { method, headers: { 'Content-Type': 'application/json' }, body: JSON.stringify(body) })
  }
  async stop(): Promise<void> {
    const child = this.process
    if (!child || child.pid === undefined) return
    this.process = undefined
    if (child.exitCode !== null || child.signalCode !== null) return
    const exited = once(child, 'exit')
    child.kill('SIGTERM')
    const timer = setTimeout(() => child.kill('SIGKILL'), 5_000)
    try { await exited } finally { clearTimeout(timer) }
  }
  async dispose(): Promise<void> {
    await this.stop()
    if (this.dir) await rm(this.dir, { recursive: true, force: true })
  }
}
