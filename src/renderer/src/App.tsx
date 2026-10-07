import { lazy, Suspense, useCallback, useEffect, useState } from 'react'
import type { IndexStatus, Settings } from '../../shared/types'
import { isMod } from './lib/util'
import Search from './components/Search'

// Rarely opened, so they stay out of the first paint.
const SettingsView = lazy(() => import('./components/Settings'))
const Onboarding = lazy(() => import('./components/Onboarding'))

const api = window.api
const THEME_KEY = 'magpie.theme'

/** Resolves `system` against the OS and applies the theme without a transition smear. */
function useTheme(theme: Settings['theme'] | undefined): void {
  useEffect(() => {
    if (!theme) return
    try {
      localStorage.setItem(THEME_KEY, theme) // read by main.tsx before first paint
    } catch {
      /* storage unavailable: the system theme is used until settings load */
    }
    const mq = matchMedia('(prefers-color-scheme: dark)')
    const apply = (): void => {
      const next = theme === 'system' ? (mq.matches ? 'dark' : 'light') : theme
      const root = document.documentElement
      if (root.dataset.theme === next) return
      const freeze = document.createElement('style')
      freeze.textContent = '*,*::before,*::after{transition:none!important}'
      document.head.append(freeze)
      root.dataset.theme = next
      void document.body.offsetHeight
      requestAnimationFrame(() => freeze.remove())
    }
    apply()
    mq.addEventListener('change', apply)
    return () => mq.removeEventListener('change', apply)
  }, [theme])
}

export default function App(): React.JSX.Element {
  const [settings, setSettings] = useState<Settings | null>(null)
  const [status, setStatus] = useState<IndexStatus | null>(null)
  const [settingsOpen, setSettingsOpen] = useState(false)

  useEffect(() => {
    api.getSettings().then(setSettings)
    api.status().then(setStatus)
    const offs = [api.onStatus(setStatus), api.onOpenSettings(() => setSettingsOpen(true))]
    return () => offs.forEach((off) => off())
  }, [])

  useEffect(() => {
    const onKey = (e: KeyboardEvent): void => {
      if (isMod(e) && e.key === ',') {
        e.preventDefault()
        setSettingsOpen(true)
      }
    }
    window.addEventListener('keydown', onKey)
    return () => window.removeEventListener('keydown', onKey)
  }, [])

  useTheme(settings?.theme)

  const update = useCallback(async (patch: Partial<Settings>) => {
    const next = await api.setSettings(patch)
    setSettings(next)
    return next
  }, [])

  // The search box paints immediately; settings arrive a moment later.
  if (settings && !settings.onboarded)
    return (
      <Suspense fallback={null}>
        <Onboarding settings={settings} onDone={update} />
      </Suspense>
    )
  return (
    <>
      <Search
        active={!settingsOpen}
        status={status}
        semantic={settings?.semantic ?? true}
        sharpText={settings?.sharpText ?? false}
        onOpenSettings={() => setSettingsOpen(true)}
      />
      {settingsOpen && settings && (
        <Suspense fallback={null}>
          <SettingsView
            settings={settings}
            status={status}
            onChange={update}
            onClose={() => setSettingsOpen(false)}
          />
        </Suspense>
      )}
    </>
  )
}
