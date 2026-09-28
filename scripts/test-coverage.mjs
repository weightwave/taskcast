// Run every workspace test project with its own coverage configuration.
// Vitest workspace mode does not apply per-project coverage thresholds.
import { readdir, access } from 'node:fs/promises'
import { spawn } from 'node:child_process'
import { fileURLToPath } from 'node:url'
import { join } from 'node:path'

const root = fileURLToPath(new URL('..', import.meta.url))
let failed = false
for (const entry of (await readdir(join(root, 'packages'), { withFileTypes: true })).sort((a, b) => a.name.localeCompare(b.name))) {
  if (!entry.isDirectory()) continue
  const cwd = join(root, 'packages', entry.name)
  try { await access(join(cwd, 'vitest.config.ts')) } catch { continue }
  const child = spawn('pnpm', ['exec', 'vitest', 'run', '--coverage', '--no-file-parallelism'], { cwd, stdio: 'inherit' })
  const code = await new Promise((resolve, reject) => { child.once('error', reject); child.once('exit', resolve) })
  if (code !== 0) failed = true
}
process.exitCode = failed ? 1 : 0
