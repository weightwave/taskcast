import path from 'path'
import tailwindcss from '@tailwindcss/vite'
import react from '@vitejs/plugin-react'
import { defineConfig } from 'vite'

export default defineConfig(({ command }) => ({
  base: command === 'build' ? '/_playground/' : '/',
  plugins: [react(), tailwindcss()],
  resolve: {
    alias: {
      '@': path.resolve(__dirname, './src'),
    },
  },
  server: {
    proxy: {
      '/taskcast': {
        target: 'http://localhost:3721',
        changeOrigin: true,
      },
      '/workers': {
        target: 'http://localhost:3721',
        changeOrigin: true,
        ws: true,
        rewrite: (path) => `/taskcast${path}`,
      },
    },
  },
}))
