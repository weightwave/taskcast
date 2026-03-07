import { defineConfig } from '@playwright/test'

export default defineConfig({
  testDir: './browser',
  timeout: 30_000,
  retries: 0,
  use: {
    baseURL: 'http://localhost:3722',
    headless: true,
  },
  projects: [
    { name: 'chromium', use: { browserName: 'chromium' } },
  ],
  webServer: {
    command: 'node --import tsx browser/start-servers.ts',
    port: 3722,
    timeout: 15_000,
    reuseExistingServer: !process.env.CI,
  },
})