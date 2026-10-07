import { Fragment, useEffect, useRef, useState } from 'react'
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
import {
  accelerator,
  basename,
  FILTERS,
  isMac,
  num,
  plural,
  trashKey,
  trashName
} from '../lib/util'
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
  initialTab?: Tab
}

export default function SettingsView(props: Props): React.JSX.Element {
  const { onClose } = props
  const [saveError, setSaveError] = useState('')
  const safeProps = {
    ...props,
    onChange: async (patch: Partial<Settings>): Promise<Settings> => {
      try {
        const next = await props.onChange(patch)
        setSaveError('')
        return next
      } catch (e) {
        setSaveError(`Couldn’t save settings: ${String(e)}`)
        return props.settings
      }
    }
  }
  const [tab, setTab] = useState<Tab>(props.initialTab ?? 'library')
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
          {saveError && (
            <p className="error" role="alert">
              {saveError}
            </p>
          )}
          {tab === 'library' && <Library {...safeProps} />}
          {tab === 'search' && <SearchTab {...safeProps} />}
          {tab === 'general' && <General {...safeProps} recordingRef={recordingRef} />}
          {tab === 'shortcuts' && <Shortcuts settings={props.settings} />}
        </div>
      </div>
    </div>
  )
}

// ---------------------------------------------------------------- library

function Library({ settings, status, onChange }: Props): React.JSX.Element {
  const [error, setError] = useState('')
  const [stats, setStats] = useState<Stats | null>(null)
  /** A folder (or the whole-computer scope) waiting for "forget" to be confirmed. */
  const [removing, setRemoving] = useState<string | null>(null)
  const total = status?.total

  useEffect(() => {
    api
      .stats()
      .then(setStats)
      .catch((e) => setError(String(e)))
  }, [settings.folders, total])

  const add = async (): Promise<void> => {
    try {
      const p = await api.pickFolder()
      if (p && !settings.folders.includes(p)) await onChange({ folders: [...settings.folders, p] })
    } catch (e) {
      setError(String(e))
    }
  }
  const counts = new Map(stats?.folders.map((f) => [f.path, f.count]))

  const everywhere = settings.scope === 'everywhere'

  return (
    <>
      <h2 className="panel-title">Library</h2>
      {error && (
        <p role="alert" className="error">
          {error}
        </p>
      )}
      <section className="group">
        <h3 id="scope-label">Where to look</h3>
        <ScopeChoice
          value={settings.scope}
          // Narrowing forgets everything outside the folders, which can take hours to read back.
          onChange={(scope) => (scope === 'folders' ? setRemoving('*') : onChange({ scope }))}
          labels={['Chosen folders', 'Everywhere on this computer']}
          foldersHint="Only the folders listed below."
          labelledBy="scope-label"
        />
        {removing === '*' && (
          <div className="confirm" role="alert">
            <p>
              Magpie forgets every image outside your folders. Switching back reads them all again.
            </p>
            <div className="btn-row">
              <button
                className="btn danger small"
                onClick={() => {
                  setRemoving(null)
                  onChange({ scope: 'folders' })
                }}
              >
                Only chosen folders
              </button>
              <button className="btn ghost small" onClick={() => setRemoving(null)}>
                Cancel
              </button>
            </div>
          </div>
        )}
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
              {removing === f ? (
                <span className="btn-row" role="alert">
                  <button
                    className="btn danger small"
                    autoFocus
                    onClick={() => {
                      setRemoving(null)
                      onChange({ folders: settings.folders.filter((x) => x !== f) })
                    }}
                  >
                    {counts.get(f) ? `Forget ${plural(counts.get(f)!, 'image')}` : 'Remove'}
                  </button>
                  <button className="btn ghost small" onClick={() => setRemoving(null)}>
                    Cancel
                  </button>
                </span>
              ) : (
                <>
                  {counts.has(f) && (
                    <span className="row-value">{plural(counts.get(f)!, 'image')}</span>
                  )}
                  <button
                    className="icon-btn"
                    aria-label={`Stop watching ${f}`}
                    title="Remove"
                    onClick={() => setRemoving(f)}
                  >
                    <X size={14} strokeWidth={1.75} />
                  </button>
                </>
              )}
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
        <h3>Excluded folders</h3>
        <p className="group-hint">
          Images in these folders and their subfolders are left out of the index, including when
          searching everywhere.
        </p>
        <ul className="list">
          {settings.excludedFolders.map((f) => (
            <li className="row" key={f}>
              <span className="row-text path" title={f}>
                {f}
              </span>
              <button
                className="icon-btn"
                aria-label={`Stop excluding ${f}`}
                onClick={() =>
                  onChange({ excludedFolders: settings.excludedFolders.filter((v) => v !== f) })
                }
              >
                <X size={14} />
              </button>
            </li>
          ))}
        </ul>
        <button
          className="btn"
          onClick={async () => {
            try {
              const folder = await api.pickFolder()
              if (folder && !settings.excludedFolders.includes(folder))
                await onChange({ excludedFolders: [...settings.excludedFolders, folder] })
            } catch (e) {
              setError(String(e))
            }
          }}
        >
          Exclude folder…
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
        {/* The raw error is for bug reports; the hint says what to do. */}
        {model.state === 'error' && (
          <span className="row-hint" title={model.error}>
            Couldn’t load. Use Repair models below to retry the download.
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
  const [message, setMessage] = useState('')
  const [busy, setBusy] = useState(false)
  const [confirmClear, setConfirmClear] = useState(false)
  const [failures, setFailures] = useState<Awaited<ReturnType<typeof api.failures>>>([])
  useEffect(() => {
    api
      .failures()
      .then(setFailures)
      .catch((e) => setMessage(String(e)))
  }, [status?.errors])
  const run = async (action: () => Promise<unknown>, success: string): Promise<void> => {
    setBusy(true)
    try {
      await action()
      if (success) setMessage(success)
    } catch (e) {
      setMessage(`Couldn’t complete action: ${String(e)}`)
    } finally {
      setBusy(false)
    }
  }
  return (
    <>
      <h2 className="panel-title">Search</h2>
      {message && (
        <p role="status" className="group-hint">
          {message}
        </p>
      )}
      <section className="group">
        <Switch
          checked={settings.semantic}
          onChange={(v) => onChange({ semantic: v })}
          label="Search by what images show"
          hint="Find shots like “sunset” or “receipt” even when they have no matching text. Downloads a 150 MB model once. Images never leave this device."
        />
        {settings.semantic && model && (
          <ModelRow label="Visual model" model={model} ready="Model loaded" />
        )}
        {settings.semantic && status && (
          <div className="coverage">
            <progress
              max={Math.max(1, status.total)}
              value={status.embedded}
              aria-label="Visual search coverage"
            />
            <p>
              {num(status.embedded)} of {num(status.total)} images searchable by look (
              {status.total ? Math.round((status.embedded / status.total) * 100) : 0}%).
            </p>
            <p className="group-hint">
              A loaded model can search only images already indexed. {status.waitingReason || ''}
            </p>
          </div>
        )}
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
            onClick={() =>
              run(() => api.pause(!paused), paused ? 'Indexing resumed' : 'Indexing paused')
            }
            disabled={busy || !status || status.state === 'idle'}
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
              void run(async () => {
                await api.reindex()
                setRebuilt(true)
              }, 'Rebuild requested')
            }}
          >
            <RotateCw size={14} strokeWidth={1.75} aria-hidden />
            Rebuild index
          </button>
        </div>
        <p className="group-hint" role="status">
          {rebuilt
            ? 'Rebuild requested. Progress is shown above.'
            : 'Rebuilding reads every image again. Use it if results look out of date. Rebuilding and retrying run with priority and may increase CPU use and battery consumption.'}
        </p>
      </section>
      <section className="group">
        <h3>Finish indexing</h3>
        <p className="group-hint">
          Continue while you’re using the computer, including on battery. This may increase CPU use,
          fan noise and power consumption. Normal background behavior returns when indexing
          finishes.
        </p>
        <button
          className="btn"
          disabled={busy || !status || status.total === 0}
          onClick={() =>
            run(
              () => api.finishIndexing(!status?.forceIndexing),
              status?.forceIndexing
                ? 'Normal background indexing restored'
                : 'Priority indexing requested'
            )
          }
        >
          {status?.forceIndexing ? 'Use normal background indexing' : 'Finish indexing now'}
        </button>
      </section>
      <section className="group">
        <h3>Recovery</h3>
        {status?.warnings?.map((warning) => (
          <p className="group-hint" key={warning} role="status">
            {warning}
          </p>
        ))}
        {failures.length > 0 && (
          <ul className="list">
            {failures.map((f) => (
              <li className="row" key={f.id}>
                <span className="row-text">
                  <span className="row-label">{f.name}</span>
                  <span className="row-hint">{f.error}</span>
                </span>
              </li>
            ))}
          </ul>
        )}
        <div className="btn-row">
          <button
            className="btn"
            disabled={busy || !status?.errors}
            onClick={() => run(api.retryFailed, 'Failed files queued for another attempt')}
          >
            Retry failed files
          </button>
          <button
            className="btn"
            disabled={busy}
            onClick={() =>
              run(api.repairModels, 'Model repair requested. A download may be needed.')
            }
          >
            Repair models
          </button>
          <button
            className="btn"
            disabled={busy}
            onClick={() =>
              run(async () => {
                const path = await api.exportDiagnostics()
                if (path) setMessage(`Diagnostics saved to ${path}`)
              }, '')
            }
          >
            Export diagnostics…
          </button>
        </div>
        <p className="group-hint">
          Search and indexing run locally. Diagnostic exports contain aggregate status and settings
          flags, without image paths, OCR text, notes or search queries.
        </p>
        {!confirmClear ? (
          <button className="btn ghost" disabled={busy} onClick={() => setConfirmClear(true)}>
            Clear local index…
          </button>
        ) : (
          <div className="confirm" role="alert">
            <p>
              Clear Glint’s local index, extracted text, metadata and feedback? Your original images
              stay in place. Watched folders and clipboard saving will be turned off. Choose folders
              again to start a new index.
            </p>
            <div className="btn-row">
              <button
                className="btn danger"
                disabled={busy}
                onClick={() =>
                  run(async () => {
                    await api.clearIndex()
                    await onChange({})
                    setConfirmClear(false)
                  }, 'Local index cleared. Choose folders in Library to start again.')
                }
              >
                Clear local index
              </button>
              <button className="btn ghost" onClick={() => setConfirmClear(false)}>
                Cancel
              </button>
            </div>
          </div>
        )}
      </section>
      <section className="group">
        <h3>Saved searches</h3>
        <ul className="list">
          {settings.savedSearches.map((saved) => (
            <li className="row" key={saved.id}>
              <span className="row-text">
                <span className="row-label">{saved.name}</span>
                <span className="row-hint">
                  {saved.mode} · {saved.query}
                </span>
              </span>
              <button
                className="icon-btn"
                aria-label={`Remove saved search ${saved.name}`}
                onClick={() =>
                  onChange({
                    savedSearches: settings.savedSearches.filter((s) => s.id !== saved.id)
                  })
                }
              >
                <X size={14} />
              </button>
            </li>
          ))}
        </ul>
        {!settings.savedSearches.length && (
          <p className="group-hint">Save a search from the search bar to use it here again.</p>
        )}
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
            <span className="row-hint">Puts the newest image on the clipboard, from any app</span>
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
          [settings.copyLatestHotkey, 'Copy the newest image to the clipboard, from any app'] as [
            string,
            string
          ]
        ]
      : []),
    ['Up+Down+Left+Right', 'Move through results'],
    ['Enter', 'Open the selected shot'],
    ['Mod+C', 'Copy its text'],
    ['Mod+Shift+C', 'Copy the image'],
    ['Mod+F', 'Go to the search box'],
    ['Mod+Shift+F', 'Find similar images'],
    ['Mod+P', 'Pin or unpin'],
    ['Mod+E', 'Show every shot in a burst'],
    ['Mod+L', 'Switch between grid and list'],
    ['Mod+Shift+A', 'Select all results'],
    [trashKey(), `Move selected to the ${trashName()}`],
    ['Mod+Z', 'Undo the last move'],
    ['Mod+,', 'Settings'],
    [
      'Esc',
      settings.hideOnBlur
        ? 'Go back, clear the search, then hide'
        : 'Go back, then clear the search'
    ]
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
      <h2 className="panel-title sub">Search filters</h2>
      <p className="group-hint">Type these in the search box, alone or with words.</p>
      <dl className="syntax">
        {FILTERS.map(([token, desc]) => (
          <Fragment key={token}>
            <dt>
              <code>{token}</code>
            </dt>
            <dd>{desc}</dd>
          </Fragment>
        ))}
      </dl>
    </>
  )
}
