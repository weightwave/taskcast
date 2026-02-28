import { defineConfig } from 'vitest/config'

export default defineConfig({
  test: {
    include: ['rust/tests/**/*.test.ts'],
  },
})
