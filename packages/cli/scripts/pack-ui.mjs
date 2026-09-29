import { access, cp, rm } from 'node:fs/promises'
import { resolve } from 'node:path'

// pnpm build prepares both private UI workspaces before ci:publish packs the CLI.
// Ship only their built static files; consumers never install the private packages.
for (const [workspace, directory] of [['dashboard-web', 'dashboard'], ['playground', 'playground']]) {
  const source = resolve('..', workspace, 'dist')
  const destination = resolve('dist', 'ui', directory)
  await access(resolve(source, 'index.html'))
  await rm(destination, { recursive: true, force: true })
  await cp(source, destination, { recursive: true })
}
