import { fileURLToPath, URL } from 'node:url'

import tailwindcss from '@tailwindcss/vite'
import vue from '@vitejs/plugin-vue'
import { defineConfig } from 'vite'

// `sych serve` serves the build at /ui on its own origin; `pnpm dev` proxies the API to it.
const server = process.env.SYCH_URL ?? 'http://127.0.0.1:8080'

export default defineConfig({
  base: '/ui/',
  plugins: [vue(), tailwindcss()],
  resolve: {
    alias: {
      '@': fileURLToPath(new URL('./src', import.meta.url)),
    },
  },
  server: {
    proxy: {
      '/v1': server,
      '/auth': server,
    },
  },
})
