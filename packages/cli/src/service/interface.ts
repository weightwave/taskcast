// packages/cli/src/service/interface.ts

export interface ServiceInstallOptions {
  port: number
  config?: string      // Absolute path to config file
  storage?: string
  dbPath?: string
  nodePath: string     // Absolute path to node executable
  entryPoint: string   // Absolute path to taskcast CLI entry
}

export type ServiceStatus =
  | { state: 'running'; pid: number; port?: number }
  | { state: 'stopped' }
  | { state: 'not-installed' }

export interface ServiceManager {
  install(opts: ServiceInstallOptions): Promise<void>
  uninstall(): Promise<void>
  start(): Promise<void>
  stop(): Promise<void>
  restart(): Promise<void>
  status(): Promise<ServiceStatus>
}
