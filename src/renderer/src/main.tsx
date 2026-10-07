import './styles.css'
import { StrictMode } from 'react'
import { createRoot } from 'react-dom/client'

// Electron's preload sets window.api. In Tauri, talk to Rust; in a plain browser (npm run dev:web), fake it.
if (!window.api) {
  window.api =
    '__TAURI_INTERNALS__' in window
      ? (await import('./lib/tauriApi')).createTauriApi()
      : (await import('./lib/mockApi')).createMockApi()
}

// Paint in the right theme from the first frame: App stores the user's choice here.
let theme: string | null = null
try {
  theme = localStorage.getItem('magpie.theme')
} catch {
  /* storage unavailable: fall back to the system theme */
}
const root = document.documentElement
root.dataset.platform = window.api.platform

// An app, not a web page: no browser right-click menu, reload, print, save or find bar.
// The app's own Ctrl+F and Ctrl+P handlers still run; they don't check defaultPrevented.
addEventListener('contextmenu', (e) => {
  if (!(e.target as Element).closest('input, textarea, .selectable')) e.preventDefault()
})
addEventListener(
  'keydown',
  (e) => {
    const k = e.key.toLowerCase()
    if (/^f[357]$/.test(k) || ((e.ctrlKey || e.metaKey) && /^[rpsfgj]$/.test(k)))
      e.preventDefault()
  },
  true
)
root.dataset.theme =
  theme === 'dark' || theme === 'light'
    ? theme
    : matchMedia('(prefers-color-scheme: light)').matches
      ? 'light'
      : 'dark'

const { default: App } = await import('./App')

createRoot(document.getElementById('root')!).render(
  <StrictMode>
    <App />
  </StrictMode>
)
