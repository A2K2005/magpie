import { useEffect, useState } from 'react'
import { ArrowLeft, FolderPlus } from 'lucide-react'
import type { FolderSuggestion, Settings } from '../../../shared/types'
import { basename, num, plural } from '../lib/util'
import { Kbd, ScopeChoice } from './ui'

const api = window.api

export default function Onboarding(props: {
  settings: Settings
  onDone(patch: Partial<Settings>): void
}): React.JSX.Element {
  const { settings, onDone } = props
  const [step, setStep] = useState(0)
  const [suggestions, setSuggestions] = useState<FolderSuggestion[] | null>(null)
  const [folders, setFolders] = useState<string[]>(settings.folders)
  const [scope, setScope] = useState(settings.scope)
  const [semantic, setSemantic] = useState(true)
  const [error, setError] = useState('')
  const [moved, setMoved] = useState(false)

  // Each step is a new screen: move focus to its heading so keyboard and screen reader users land there.
  useEffect(() => {
    if (step) document.querySelector<HTMLElement>('.onboarding h1')?.focus()
  }, [step])

  useEffect(() => {
    api.suggestFolders().then((s) => {
      setSuggestions(s)
      setFolders((f) => (f.length ? f : s.filter((x) => x.count > 0).map((x) => x.path)))
    })
  }, [])

  const toggle = (p: string, on: boolean): void => {
    setError('')
    setFolders((f) => (on ? [...f, p] : f.filter((x) => x !== p)))
  }
  const add = async (): Promise<void> => {
    const p = await api.pickFolder()
    if (!p) return
    setError('')
    setFolders((f) => (f.includes(p) ? f : [...f, p]))
    setSuggestions((s) =>
      s && !s.some((x) => x.path === p) ? [...s, { path: p, count: -1, label: basename(p) }] : s
    )
  }
  const next = (): void => {
    if (step === 0 && scope === 'folders' && folders.length === 0)
      return setError('Choose at least one folder to continue.')
    setMoved(true)
    setStep(step + 1)
  }

  return (
    <div className="onboarding">
      <div className="onboarding-drag" />
      <div className="onboarding-inner" key={step} data-moved={moved || undefined}>
        <p className="step">Step {step + 1} of 3</p>

        {step === 0 && (
          <>
            <h1 tabIndex={-1} id="scope-label">
              Where should Magpie look?
            </h1>
            <p className="lede">
              Magpie reads images on this device only, and keeps watching for new ones.
            </p>
            <ScopeChoice
              value={scope}
              onChange={(v) => {
                setError('')
                setScope(v)
              }}
              labels={['My screenshots', 'Every image on this computer']}
              foldersHint="Only the folders you choose below."
              labelledBy="scope-label"
            />
          </>
        )}

        {step === 0 && scope === 'folders' && (
          <>
            <ul className="choices" aria-label="Folders">
              {suggestions === null && (
                <li className="row muted">Looking for screenshot folders…</li>
              )}
              {suggestions?.map((s) => (
                <li key={s.path}>
                  <label className="row choice">
                    <input
                      type="checkbox"
                      className="check"
                      checked={folders.includes(s.path)}
                      onChange={(e) => toggle(s.path, e.target.checked)}
                    />
                    <span className="row-text">
                      <span className="row-label">{s.label}</span>
                      <span className="row-hint path" title={s.path}>
                        {s.path}
                      </span>
                    </span>
                    {s.count >= 0 && (
                      <span className="row-value">
                        {s.count ? plural(s.count, 'image') : 'Empty'}
                      </span>
                    )}
                  </label>
                </li>
              ))}
            </ul>
            <button className="btn ghost" onClick={add}>
              <FolderPlus size={14} strokeWidth={1.75} aria-hidden />
              Add another folder…
            </button>
          </>
        )}

        {step === 1 && (
          <>
            <h1 tabIndex={-1}>Search by what images show?</h1>
            <p className="lede">
              Magpie always finds the words in your images. It can also learn what they look
              like, so “sunset” or “receipt” works even when the words aren’t there.
            </p>
            <div className="choices" role="radiogroup" aria-label="Search mode">
              <label className="row choice card">
                <input
                  type="radio"
                  className="radio"
                  name="mode"
                  checked={semantic}
                  onChange={() => setSemantic(true)}
                />
                <span className="row-text">
                  <span className="row-label">Words and pictures</span>
                  <span className="row-hint">
                    Downloads a 150 MB model once. It runs on this device, and your images never
                    leave it.
                  </span>
                </span>
              </label>
              <label className="row choice card">
                <input
                  type="radio"
                  className="radio"
                  name="mode"
                  checked={!semantic}
                  onChange={() => setSemantic(false)}
                />
                <span className="row-text">
                  <span className="row-label">Words only</span>
                  <span className="row-hint">
                    Nothing to download. You can turn pictures on later in Settings.
                  </span>
                </span>
              </label>
            </div>
          </>
        )}

        {step === 2 && (
          <>
            <h1 tabIndex={-1}>You’re set</h1>
            <p className="lede">
              {scope === 'everywhere' ? (
                'Magpie is finding every image on this computer. You can search by name right away; text and visual search fill in as it reads them.'
              ) : (
                <>
                  Magpie is indexing {plural(folders.length, 'folder')} in the background
                  {suggestions && folders.length > 0 && countOf(suggestions, folders) > 0
                    ? `, about ${num(countOf(suggestions, folders))} images`
                    : ''}
                  .
                </>
              )}{' '}
              Open it from any app with:
            </p>
            <div className="hero-keys">
              <Kbd combo={settings.hotkey} />
            </div>
            <p className="muted">You can change the shortcut in Settings.</p>
          </>
        )}

        <p className="form-error" role="alert">
          {error}
        </p>

        <div className="onboarding-actions">
          {step > 0 && (
            <button
              className="btn ghost"
              onClick={() => {
                setMoved(true)
                setStep(step - 1)
              }}
            >
              <ArrowLeft size={14} strokeWidth={1.75} aria-hidden />
              Back
            </button>
          )}
          {step < 2 ? (
            <button className="btn primary" onClick={next}>
              Continue
            </button>
          ) : (
            <button
              className="btn primary"
              onClick={() => onDone({ folders, scope, semantic, onboarded: true })}
            >
              Start searching
            </button>
          )}
        </div>
      </div>
    </div>
  )
}

const countOf = (s: FolderSuggestion[], folders: string[]): number =>
  s.filter((x) => folders.includes(x.path) && x.count > 0).reduce((n, x) => n + x.count, 0)
