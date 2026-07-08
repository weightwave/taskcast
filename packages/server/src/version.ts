import { createRequire } from 'node:module'

const require = createRequire(import.meta.url)
const pkg = require('../package.json') as { version: string }

export const TASKCAST_SERVER_NAME = 'taskcast'
export const TASKCAST_API_VERSION = 'v1'
export const TASKCAST_SERVER_VERSION = pkg.version

export function serverInfo() {
  return {
    name: TASKCAST_SERVER_NAME,
    version: TASKCAST_SERVER_VERSION,
    apiVersion: TASKCAST_API_VERSION,
  }
}
