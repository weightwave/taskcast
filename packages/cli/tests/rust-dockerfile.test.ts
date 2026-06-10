import { readFile } from 'node:fs/promises'
import { describe, expect, it } from 'vitest'

describe('Rust Dockerfile', () => {
  it('pins the Rust builder to the runtime Debian suite', async () => {
    const dockerfile = await readFile(
      new URL('../../../rust/Dockerfile', import.meta.url),
      'utf8',
    )

    const builderTag = dockerfile.match(/^FROM\s+rust:([^\s]+)\s+AS\s+chef$/m)?.[1]
    const runtimeTag = dockerfile.match(/^FROM\s+debian:([^\s]+)\s+AS\s+runtime$/m)?.[1]

    expect(builderTag).toBeDefined()
    expect(runtimeTag).toBeDefined()

    const runtimeSuite = runtimeTag?.replace(/-slim$/, '')
    expect(builderTag).toContain(runtimeSuite)
  })
})
