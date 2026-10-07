import { useEffect, useRef, useState } from 'react'
import {
  Folder,
  FolderPlus,
  Images,
  Keyboard,
  Pause,
  Play,
  RotateCw,
  Search,
  SlidersHorizontal,
  X
} from 'lucide-react'
import type { IndexStatus, Settings, Stats } from '../../../shared/types'
import { accelerator, basename, bytes, isMac, num, plural, trashKey } from '../lib/util'
import { Kbd, ScopeChoice, Switch } from './ui'

const api = window.api

type Tab = 'library' | 'search' | 'general' | 'shortcuts'
const TABS: { id: Tab; label: string; icon: typeof Folder }[] = [
  { id: 'library', label: 'Library', icon: Images },
  { id: 'search', label: 'Search', icon: Search },
  { id: 'general', label: 'General', icon: SlidersHorizontal },
  { id: 'shortcuts', label: 'Shortcuts', icon: Keyboard }
]

interface Props {
  settings: Settings
  status: IndexStatus | null
  /** Resolves with what was actually saved (main may refuse a hotkey). */
  onChange(patch: Partial<Settings>): Promise<Settings>
  onClose(): void
}

export default function SettingsView(props: Props): React.JSX.Element {
  const { onClose } = props
  const [tab, setTab] = useState<Tab>('library')
  const recordingRef = useRef(false)
  const root = useRef<HTMLDivElement>(null)

  useEffect(() => {
    root.current?.focus()
    const onKey = (e: KeyboardEvent): void => {
      if (e.key === 'Escape' && !recordingRef.current) {
        e.preventDefault()
        onClose()
      }
    }
    window.addEventListener('keydown', onKey)
    return () => window.removeEventListener('keydown', onKey)
  }, [onClose])

  const onTabKey = (e: React.KeyboardEvent): void => {
    const i = TABS.findIndex((t) => t.id === tab)
    // Up/Down, or Left/Right when a narrow window lays the tabs out across the top.
    const next =
      e.key === 'ArrowDown' || e.key === 'ArrowRight'
        ? i + 1
        : e.key === 'ArrowUp' || e.key === 'ArrowLeft'
          ? i - 1
          : e.key === 'Home'
            ? 0
            : e.key === 'End'
              ? TABS.length - 1
              : -2
    if (next === -2) return
    e.preventDefault()
    const t = TABS[(next + TABS.length) % TABS.length]
    setTab(t.id)
    document.getElementById(`tab-${t.id}`)?.focus()
  }

  return (
    <div
      className="settings"
      role="dialog"
      aria-modal
      aria-label="Settings"
      ref={root}
      tabIndex={-1}
    >
      <header className="bar">
        <span className="bar-title strong">Settings</span>
        <button className="btn" onClick={onClose}>
          Done
        </button>
      </header>
      <div className="settings-body">
        <nav
          className="settings-nav"
          role="tablist"
          aria-orientation="vertical"
          onKeyDown={onTabKey}
        >
          {TABS.map((t) => (
            <button
              key={t.id}
              id={`tab-${t.id}`}
              role="tab"
              aria-selected={tab === t.id}
              aria-controls="settings-panel"
              tabIndex={tab === t.id ? 0 : -1}
              className="nav-item"
              onClick={() => setTab(t.id)}
            >
              <t.icon size={15} strokeWidth={1.75} aria-hidden />
              {t.label}
            </button>
          ))}
        </nav>
        <div
          className="settings-panel"
          id="settings-panel"
          role="tabpanel"
          aria-labelledby={`tab-${tab}`}
        >
          {tab === 'library' && <Library {...props} />}
          {tab === 'search' && <SearchTab {...props} />}
          {tab === 'general' && <General {...props} recordingRef={recordingRef} />}
          {tab === 'shortcuts' && <Shortcuts settings={props.settings} />}
        </div>
      </div>
    </div>
  )
}

// ---------------------------------------------------------------- library

const fmtMonth = new Intl.DateTimeFormat(undefined, { month: 'short' })
const fmtMonthYear = new Intl.DateTimeFormat(undefined, { month: 'long', year: 'numeric' })
const monthDate = (m: string): Date => new Date(Number(m.slice(0, 4)), Number(m.slice(5, 7)) - 1, 1)

function Library({ settings, status, onChange }: Props): React.JSX.Element {
  const [stats, setStats] = useState<Stats | null>(null)
  const total = status?.total

  useEffect(() => {
    api.stats().then(setStats)
  }, [settings.folders, total])

  const add = async (): Promise<void> => {
    const p = await api.pickFolder()
    if (p && !settings.folders.includes(p)) onChange({ folders: [...settings.folders, p] })
  }
  const counts = new Map(stats?.folders.map((f) => [f.path, f.count]))
  const max = Math.max(1, ...(stats?.months.map((m) => m.count) ?? []))

  const everywhere = settings.scope === 'everywhere'

  return (
    <>
      <h2 className="panel-title">Library</h2>
      <section className="group">
        <h3 id="scope-label">Where to look</h3>
        <ScopeChoice
          value={settings.scope}
          onChange={(scope) => onChange({ scope })}
          labels={['Chosen folders', 'Everywhere on this computer']}
          foldersHint="Only the folders listed below."
          labelledBy="scope-label"
        />
      </section>

      <section className="group">
        <h3>{everywhere ? 'Extra folders' : 'Folders'}</h3>
        <p className="group-hint">
          {everywhere
            ? 'Magpie also watches these, for example a folder it would otherwise skip.'
            : 'Magpie watches these folders and indexes new images as they arrive.'}
        </p>
        <ul className="list">
          {settings.folders.map((f) => (
            <li className="row" key={f}>
              <Folder size={15} strokeWidth={1.75} className="row-icon" aria-hidden />
              <span className="row-text">
                <span className="row-label">{basename(f)}</span>
                <span className="row-hint path" title={f}>
                  {f}
                </span>
              </span>
              {counts.has(f) && <span className="row-value">{plural(counts.get(f)!, 'shot')}</span>}
              <button
                className="icon-btn"
                aria-label={`Stop watching ${f}`}
                title="Remove"
                onClick={() => onChange({ folders: settings.folders.filter((x) => x !== f) })}
              >
                <X size={14} strokeWidth={1.75} />
              </button>
            </li>
          ))}
          {settings.folders.length === 0 && (
            <li className="row muted">
              {everywhere
                ? 'None. Add one only if Magpie skips a folder you need.'
                : 'No folders yet. Add one to start indexing.'}
            </li>
          )}
        </ul>
        <button className="btn" onClick={add}>
          <FolderPlus size={14} strokeWidth={1.75} aria-hidden />
          Add folder…
        </button>
      </section>

      <section className="group">
        <Switch
          checked={settings.saveClipboard}
          onChange={(v) => onChange({ saveClipboard: v })}
          label="Save copied images"
          hint="Images you copy are saved to Pictures › Magpie Clipboard, so you can find them later."
        />
      </section>

      {stats && (
        <section className="group">
          <h3>Overview</h3>
          <div className="figures">
            <div className="figure">
              <span className="figure-value">{num(stats.total)}</span>
              <span className="figure-label">Images</span>
            </div>
            <div className="figure">
              <span className="figure-value">{num(stats.withText)}</span>
              <span className="figure-label">
                With text
                {stats.total > 0 && ` · ${Math.round((stats.withText / stats.total) * 100)}%`}
              </span>
            </div>
            <div className="figure">
              <span className="figure-value">{bytes(stats.bytes)}</span>
              <span className="figure-label">On disk</span>
            </div>
          </div>

          <div className="chart">
            <div className="chart-head">
              <span>Images per month</span>
              <span className="muted">Last 12 months</span>
            </div>
            <ol className="bars" aria-label="Images per month, last 12 months">
              {stats.months.map((m, i) => {
                const label = `${fmtMonthYear.format(monthDate(m.month))}: ${plural(m.count, 'image')}`
                return (
                  <li key={m.month} className="bar-col" aria-label={label} data-tip={num(m.count)}>
                    <span
                      className="bar-fill"
                      data-current={i === stats.months.length - 1 || undefined}
                      style={{ height: `${(m.count / max) * 100}%` }}
                    />
                    <span className="bar-label" aria-hidden>
                      {fmtMonth.format(monthDate(m.month)).slice(0, 1)}
                    </span>
                  </li>
                )
              })}
            </ol>
          </div>

          {stats.colors.length > 0 && (
            <div className="top-colors">
              <span className="chart-head">Most common colors</span>
              <ul className="color-row">
                {stats.colors.map((c) => (
                  <li key={c.hex} title={`${c.hex} · ${plural(c.count, 'image')}`}>
                    <span className="swatch lg" style={{ background: c.hex }} aria-hidden />
                    <span className="color-count">{num(c.count)}</span>
                    <span className="sr-only">{c.hex}</span>
                  </li>
                ))}
              </ul>
            </div>
          )}
        </section>
      )}
    </>
  )
}

// ---------------------------------------------------------------- search

const MODEL_TEXT: Record<string, string> = {
  off: 'Off',
  downloading: 'Downloading',
  loading: 'Loading',
  ready: 'Ready',
  error: 'Couldn’t load'
}

/** A downloadable model: state, download progress, or why it failed. */
function ModelRow(props: {
  label: string
  model: IndexStatus['model']
  /** Shown instead of "Ready". */
  ready?: string
}): React.JSX.Element {
  const { label, model } = props
  const pct = Math.round((model.progress ?? 0) * 100)
  return (
    <div className="row">
      <span className="row-text">
        <span className="row-label">{label}</span>
        {model.state === 'error' && (
          <span className="row-hint">
            {model.error ?? 'Turn the setting off and on to try again.'}
          </span>
        )}
      </span>
      {model.state === 'downloading' && (
        <span
          className="progress"
          role="progressbar"
          aria-label={`${label} download`}
          aria-valuenow={pct}
        >
          <span style={{ width: `${pct}%` }} />
        </span>
      )}
      <span className="row-value">
        {model.state === 'ready' && props.ready ? props.ready : MODEL_TEXT[model.state]}
        {model.state === 'downloading' && ` ${pct}%`}
      </span>
    </div>
  )
}

function SearchTab({ settings, status, onChange }: Props): React.JSX.Element {
  const model = status?.model
  const paused = status?.state === 'paused'
  const [rebuilt, setRebuilt] = useState(false)
  return (
    <>
      <h2 className="panel-title">Search</h2>
      <section className="group">
        <Switch
          checked={settings.semantic}
          onChange={(v) => onChange({ semantic: v })}
          label="Search by what images show"
          hint="Find shots like “sunset” or “receipt” even when they have no matching text. Downloads a 150 MB model once. Images never leave this device."
        />
        {settings.semantic && model && <ModelRow label="Visual model" model={model} />}
      </section>

      {api.platform === 'win32' && (
        <section className="group">
          <Switch
            checked={settings.sharpText}
            onChange={(v) => onChange({ sharpText: v })}
            label="Sharper text"
            hint="Re-reads the text in your images in the background with a more accurate model. Downloads 13 MB once."
          />
          {settings.sharpText && status?.textModel && (
            <ModelRow
              label="Text model"
              model={status.textModel}
              ready={
                status.sharp < status.ocrDone
                  ? `Sharpened ${num(status.sharp)} of ${num(status.ocrDone)}`
                  : 'All text sharpened'
              }
            />
          )}
        </section>
      )}

      <section className="group">
        <h3>Index</h3>
        {status && (
          <ul className="list">
            {(
              [
                ['Images found', status.total],
                ['Searchable by text', status.ocrDone],
                ...(settings.semantic ? [['Searchable by look', status.embedded]] : []),
                ...(status.errors ? [['Couldn’t be read', status.errors]] : [])
              ] as [string, number][]
            ).map(([label, n]) => (
              <li className="row" key={label}>
                <span className="row-text">{label}</span>
                <span className="row-value">{num(n)}</span>
              </li>
            ))}
          </ul>
        )}
        <div className="btn-row">
          <button
            className="btn"
            onClick={() => api.pause(!paused)}
            disabled={!status || status.state === 'idle'}
          >
            {paused ? (
              <Play size={14} strokeWidth={1.75} aria-hidden />
            ) : (
              <Pause size={14} strokeWidth={1.75} aria-hidden />
            )}
            {paused ? 'Resume indexing' : 'Pause indexing'}
          </button>
          <button
            className="btn"
            onClick={() => {
              api.reindex()
              setRebuilt(true)
            }}
          >
            <RotateCw size={14} strokeWidth={1.75} aria-hidden />
            Rebuild index
          </button>
        </div>
        <p className="group-hint" role="status">
          {rebuilt
            ? 'Rebuilding. Search keeps working with what’s already indexed.'
            : 'Rebuilding reads every image again. Use it if results look out of date.'}
        </p>
      </section>
    </>
  )
}

// ---------------------------------------------------------------- general

function General({
  settings,
  onChange,
  recordingRef
}: Props & { recordingRef: { current: boolean } }): React.JSX.Element {
  return (
    <>
      <h2 className="panel-title">General</h2>
      <section className="group">
        <div className="row">
          <span className="row-text">
            <span className="row-label" id="hotkey-label">
              Open Magpie
            </span>
            <span className="row-hint">Works from any app</span>
          </span>
          <HotkeyRecorder
            labelId="hotkey-label"
            value={settings.hotkey}
            onChange={async (hotkey) => (await onChange({ hotkey })).hotkey === hotkey}
            recordingRef={recordingRef}
          />
        </div>
        <div className="row">
          <span className="row-text">
            <span className="row-label" id="copy-latest-label">
              Copy latest image
            </span>
            <span className="row-hint">
              Puts the newest image on the clipboard, from any app
            </span>
          </span>
          <HotkeyRecorder
            labelId="copy-latest-label"
            value={settings.copyLatestHotkey}
            onChange={async (copyLatestHotkey) =>
              (await onChange({ copyLatestHotkey })).copyLatestHotkey === copyLatestHotkey
            }
            onClear={() => onChange({ copyLatestHotkey: '' })}
            recordingRef={recordingRef}
          />
        </div>
        <Switch
          checked={settings.launchAtLogin}
          onChange={(v) => onChange({ launchAtLogin: v })}
          label="Open at login"
        />
        <Switch
          checked={settings.hideOnBlur}
          onChange={(v) => onChange({ hideOnBlur: v })}
          label="Hide when you click away"
        />
      </section>
      <section className="group">
        <h3 id="theme-label">Appearance</h3>
        <div className="segmented" role="radiogroup" aria-labelledby="theme-label">
          {(['system', 'dark', 'light'] as const).map((t) => (
            <label key={t} className="segment">
              <input
                type="radio"
                name="theme"
                value={t}
                checked={settings.theme === t}
                onChange={() => onChange({ theme: t })}
              />
              <span>{t === 'system' ? 'Match system' : t === 'dark' ? 'Dark' : 'Light'}</span>
            </label>
          ))}
        </div>
      </section>
    </>
  )
}

function HotkeyRecorder(props: {
  labelId: string
  /** Empty means off. */
  value: string
  /** Offers "Turn off" when set. */
  onClear?(): void
  /** Resolves false when the shortcut could not be registered. */
  onChange(v: string): Promise<boolean>
  recordingRef: { current: boolean }
}): React.JSX.Element {
  const [on, setOn] = useState(false)
  const [error, setError] = useState('')
  const { recordingRef, onChange } = props

  useEffect(() => {
    recordingRef.current = on
    if (!on) return
    const onKey = (e: KeyboardEvent): void => {
      e.preventDefault()
      e.stopPropagation()
      if (e.key === 'Escape') return setOn(false)
      const r = accelerator(e)
      if (!r) return
      if ('error' in r) return setError(r.error)
      setError('')
      setOn(false)
      onChange(r.value).then((ok) => ok || setError('That shortcut is taken by another app'))
    }
    window.addEventListener('keydown', onKey, true)
    return () => window.removeEventListener('keydown', onKey, true)
  }, [on, recordingRef, onChange])

  return (
    <span className="hotkey">
      <span className="hotkey-controls">
        {props.onClear && props.value && !on && (
          <button className="btn ghost small" onClick={props.onClear}>
            Turn off
          </button>
        )}
        <button
          className="hotkey-btn"
          data-recording={on || undefined}
          aria-describedby={props.labelId}
          onClick={() => {
            setError('')
            setOn(!on)
          }}
          onBlur={() => setOn(false)}
        >
          {on ? 'Type a shortcut…' : props.value ? <Kbd combo={props.value} /> : 'Off · Record'}
        </button>
      </span>
      <span className="hotkey-hint" role="status" data-error={!!error || undefined}>
        {error || (on ? 'Press Esc to cancel' : '')}
      </span>
    </span>
  )
}

// ---------------------------------------------------------------- shortcuts

function Shortcuts({ settings }: { settings: Settings }): React.JSX.Element {
  const rows: [string, string][] = [
    [settings.hotkey, 'Open Magpie from anywhere'],
    ...(settings.copyLatestHotkey
      ? [
          [
            settings.copyLatestHotkey,
            'Copy the newest image to the clipboard, from any app'
          ] as [string, string]
        ]
      : []),
    ['Up+Down+Left+Right', 'Move through results'],
    ['Enter', 'Open the selected shot'],
    ['Mod+C', 'Copy its text'],
    ['Mod+Shift+C', 'Copy the image'],
    ['Mod+F', 'Find similar shots'],
    ['Mod+P', 'Pin or unpin'],
    ['Mod+E', 'Show every shot in a burst'],
    ['Mod+L', 'Switch between grid and list'],
    ['Mod+Shift+A', 'Select all results'],
    [trashKey(), 'Move selected to the trash'],
    ['Mod+Z', 'Undo the last trash'],
    ['Mod+,', 'Settings'],
    ['Esc', 'Go back, clear the search, then hide']
  ]
  return (
    <>
      <h2 className="panel-title">Shortcuts</h2>
      <p className="group-hint">
        Click a result with {isMac() ? '⌘' : 'Ctrl'} or Shift held to select several. Drop an image
        anywhere to find images that look like it.
      </p>
      <dl className="shortcuts">
        {rows.map(([combo, what]) => (
          <div className="shortcut" key={what}>
            <dt>{what}</dt>
            <dd>
              <Kbd combo={combo} />
            </dd>
          </div>
        ))}
      </dl>
    </>
  )
}
