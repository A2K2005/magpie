// Renderer as a plain web app: the Tauri build (`npm run build:web` → dist-web) and the browser
// dev loop (`npm run dev:web`, which fakes the backend with src/renderer/src/lib/mockApi.ts).
import { resolve } from 'path'
import { defineConfig } from 'vite'
import react from '@vitejs/plugin-react'

export default defineConfig({
  root: resolve(__dirname, 'src/renderer'),
  base: './',
  resolve: { alias: { '@renderer': resolve(__dirname, 'src/renderer/src') } },
  plugins: [react()],
  server: { port: 5199, strictPort: true },
  build: { outDir: resolve(__dirname, 'dist-web'), emptyOutDir: true, target: 'es2022' }
})
