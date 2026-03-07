import { readFileSync, writeFileSync, mkdirSync } from 'fs'
import { join } from 'path'
import { homedir } from 'os'

export interface NodeEntry {
  url: string
  token?: string
  tokenType?: 'jwt' | 'admin'
}

export interface NodeListEntry extends NodeEntry {
  name: string
  current: boolean
}

interface NodeConfigData {
  current: string | null
  nodes: Record<string, NodeEntry>
}

const DEFAULT_URL = 'http://localhost:3721'

export class NodeConfigManager {
  private configPath: string

  constructor(configDir?: string) {
    const dir = configDir ?? join(homedir(), '.taskcast')
    this.configPath = join(dir, 'nodes.json')
  }

  getCurrent(): NodeEntry {
    const data = this.load()
    if (data.current && data.nodes[data.current]) {
      return data.nodes[data.current]
    }
    return { url: DEFAULT_URL }
  }

  get(name: string): NodeEntry | undefined {
    const data = this.load()
    return data.nodes[name]
  }

  add(name: string, entry: NodeEntry): void {
    const data = this.load()
    data.nodes[name] = entry
    this.save(data)
  }

  remove(name: string): void {
    const data = this.load()
    if (!(name in data.nodes)) {
      throw new Error(`Node "${name}" not found`)
    }
    delete data.nodes[name]
    if (data.current === name) {
      data.current = null
    }
    this.save(data)
  }

  use(name: string): void {
    const data = this.load()
    if (!(name in data.nodes)) {
      throw new Error(`Node "${name}" not found`)
    }
    data.current = name
    this.save(data)
  }

  list(): NodeListEntry[] {
    const data = this.load()
    return Object.entries(data.nodes).map(([name, entry]) => ({
      ...entry,
      name,
      current: data.current === name,
    }))
  }

  private load(): NodeConfigData {
    try {
      const raw = readFileSync(this.configPath, 'utf-8')
      return JSON.parse(raw) as NodeConfigData
    } catch {
      return { current: null, nodes: {} }
    }
  }

  private save(data: NodeConfigData): void {
    const dir = this.configPath.replace(/[/\\][^/\\]*$/, '')
    mkdirSync(dir, { recursive: true })
    writeFileSync(this.configPath, JSON.stringify(data, null, 2), 'utf-8')
  }
}
