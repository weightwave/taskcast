import { test } from 'node:test'
import assert from 'node:assert/strict'
import { mkdtemp, mkdir, writeFile, readFile, readdir, rm } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join, resolve } from 'node:path'
import { fileURLToPath } from 'node:url'
import { execFileSync, spawnSync, spawn } from 'node:child_process'
import { createServer } from 'node:net'
import { once } from 'node:events'

const root = fileURLToPath(new URL('../../', import.meta.url))
const script = join(root, 'packages/cli/scripts/pack-ui.mjs')

test('prepack ships built assets and removes stale output', async t => {
  const dir = await mkdtemp(join(tmpdir(), 'taskcast-ui-pack-'))
  t.after(() => rm(dir, { recursive: true, force: true }))
  const cli = join(dir, 'cli')
  await mkdir(join(cli, 'dist/ui/dashboard'), { recursive: true })
  await writeFile(join(cli, 'dist/ui/dashboard/stale.js'), 'old')
  for (const name of ['dashboard-web', 'playground']) {
    await mkdir(join(dir, name, 'dist/assets'), { recursive: true })
    await writeFile(join(dir, name, 'dist/index.html'), '<html>UI</html>')
    await writeFile(join(dir, name, 'dist/assets/app.js'), 'export {}')
    await writeFile(join(dir, name, 'source.ts'), 'must not ship')
  }
  execFileSync(process.execPath, [script], { cwd: cli })
  for (const name of ['dashboard', 'playground']) {
    assert.equal(await readFile(join(cli, 'dist/ui', name, 'index.html'), 'utf8'), '<html>UI</html>')
    assert.deepEqual((await readdir(join(cli, 'dist/ui', name))).sort(), ['assets', 'index.html'])
  }
})

test('prepack fails when a UI build is missing', async t => {
  const dir = await mkdtemp(join(tmpdir(), 'taskcast-ui-missing-'))
  t.after(() => rm(dir, { recursive: true, force: true }))
  const cli = join(dir, 'cli')
  await mkdir(cli)
  const result = spawnSync(process.execPath, [script], { cwd: cli, encoding: 'utf8' })
  assert.notEqual(result.status, 0)
  assert.match(result.stderr, /ENOENT/)
})

test('CLI tarball includes both UIs without private runtime dependencies', async t => {
  const dir = await mkdtemp(join(tmpdir(), 'taskcast-cli-tarball-'))
  t.after(() => rm(dir, { recursive: true, force: true }))
  execFileSync('pnpm', ['--filter', '@taskcast/cli', 'pack', '--pack-destination', dir], { cwd: root, stdio: 'pipe' })
  const archive = resolve(dir, (await readdir(dir)).find(name => name.endsWith('.tgz')))
  const files = execFileSync('tar', ['-tzf', archive], { encoding: 'utf8' }).split('\n')
  for (const name of ['dashboard', 'playground']) {
    assert.ok(files.includes(`package/dist/ui/${name}/index.html`))
    assert.ok(files.some(path => path.startsWith(`package/dist/ui/${name}/assets/`) && path.endsWith('.js')))
  }
  assert.ok(files.includes('package/dist/ui-assets.js'))
  const manifest = JSON.parse(execFileSync('tar', ['-xOzf', archive, 'package/package.json'], { encoding: 'utf8' }))
  assert.equal(manifest.dependencies['@taskcast/dashboard-web'], undefined)
  assert.equal(manifest.dependencies['@taskcast/playground'], undefined)

  // Exercise the actual compiled paths, including the dashboard root path guard.
  const config = join(dir, 'config.json')
  await writeFile(config, JSON.stringify({ auth: { mode: 'none' } }))
  for (const [command, route, extra] of [
    ['ui', '/', []],
    ['playground', '/_playground/', []],
    ['start', '/_playground/', ['--playground', '--config', config]],
  ]) {
    const socket = createServer().listen(0, '127.0.0.1')
    await once(socket, 'listening')
    const port = socket.address().port
    await new Promise(resolve => socket.close(resolve))
    const env = { ...process.env }
    for (const name of Object.keys(env)) if (name.startsWith('TASKCAST_')) delete env[name]
    const child = spawn(process.execPath, [join(root, 'packages/cli/dist/index.js'), command, '--port', String(port), ...extra], {
      cwd: dir, env, stdio: ['ignore', 'pipe', 'pipe'],
    })
    let output = ''
    child.stdout.on('data', data => { output += data })
    child.stderr.on('data', data => { output += data })
    try {
      const base = `http://127.0.0.1:${port}`
      let response
      for (let attempt = 0; attempt < 100; attempt++) {
        assert.equal(child.exitCode, null, output)
        try { response = await fetch(base + route); break } catch { await new Promise(resolve => setTimeout(resolve, 100)) }
      }
      assert.equal(response?.status, 200, `${command}: ${output}`)
      const html = await response.text()
      const entry = html.match(/<script[^>]+src="([^"]+)"/)?.[1]
      assert.ok(entry, `${command}: missing JavaScript entry`)
      const asset = await fetch(new URL(entry, base + route))
      assert.equal(asset.status, 200, `${command}: ${entry}`)
      assert.match(asset.headers.get('content-type'), /javascript/)
    } finally {
      if (child.exitCode === null && child.signalCode === null) {
        const exited = once(child, 'exit')
        child.kill('SIGTERM')
        const timer = setTimeout(() => child.kill('SIGKILL'), 3000)
        await exited
        clearTimeout(timer)
      }
    }
  }
})
