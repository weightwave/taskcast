import { createInterface } from 'readline'
import { mkdirSync, writeFileSync } from 'fs'
import { join } from 'path'
import { homedir } from 'os'

export const DEFAULT_CONFIG_YAML = `# Taskcast configuration
# Docs: https://github.com/weightwave/taskcast

port: 3721

# auth:
#   mode: none  # none | jwt

# adapters:
#   broadcast:
#     provider: memory  # memory | redis
#     # url: redis://localhost:6379
#   shortTermStore:
#     provider: memory  # memory | redis
#     # url: redis://localhost:6379
#   longTermStore:
#     provider: postgres
#     # url: postgresql://localhost:5432/taskcast
`

export async function promptCreateGlobalConfig(): Promise<boolean> {
  if (!process.stdin.isTTY) return false

  const globalConfigPath = join(homedir(), '.taskcast', 'taskcast.config.yaml')

  return new Promise((resolve) => {
    const rl = createInterface({ input: process.stdin, output: process.stdout })
    rl.on('close', () => resolve(false))
    rl.question(
      `[taskcast] No config file found.\n? Create a default config at ${globalConfigPath}? (Y/n) `,
      (answer) => {
        const trimmed = answer.trim().toLowerCase()
        resolve(trimmed === '' || trimmed === 'y' || trimmed === 'yes')
        rl.close()
      },
    )
  })
}

export async function promptConfirm(message: string): Promise<boolean> {
  if (!process.stdin.isTTY) return false

  return new Promise((resolve) => {
    const rl = createInterface({ input: process.stdin, output: process.stdout })
    rl.on('close', () => resolve(false))
    rl.question(message, (answer) => {
      const trimmed = answer.trim().toLowerCase()
      resolve(trimmed === '' || trimmed === 'y' || trimmed === 'yes')
      rl.close()
    })
  })
}

export function createDefaultGlobalConfig(): string | null {
  const globalDir = join(homedir(), '.taskcast')
  const globalConfigPath = join(globalDir, 'taskcast.config.yaml')
  try {
    mkdirSync(globalDir, { recursive: true })
    writeFileSync(globalConfigPath, DEFAULT_CONFIG_YAML)
    console.log(`[taskcast] Created default config at ${globalConfigPath}`)
    return globalConfigPath
  } catch (err) {
    console.warn(`[taskcast] Could not create config at ${globalConfigPath}: ${(err as Error).message}`)
    return null
  }
}
