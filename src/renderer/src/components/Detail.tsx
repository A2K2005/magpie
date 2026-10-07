import { useEffect, useRef, useState } from 'react'
import type { CSSProperties } from 'react'
import {
  ArrowLeft,
  ChevronLeft,
  ChevronRight,
  Copy,
  ExternalLink,
  FolderOpen,
  Hash,
  ImageIcon,
  Link,
  Mail,
  Palette,
  Phone,
  Pin,
  PinOff,
  ScanSearch,
  Trash2,
  Download,
  Crop,
  ThumbsDown
} from 'lucide-react'
import type { LucideIcon } from 'lucide-react'
import type { OcrLine, SearchHit, ShotDetail, SmartAction } from '../../../shared/types'
import {
  basename,
  bytes,
  isMac,
  isMod,
  longDate,
  num,
  plural,
  trashKey,
  trashName
} from '../lib/util'
import type { Actions } from './Search'
import { Kbd } from './ui'

const api = window.api

const pct = (n: number): string => `${n * 100}%`
const box = (l: { x: number; y: number; w: number; h: number }): CSSProperties => ({
  left: pct(l.x),
  top: pct(l.y),
  width: pct(l.w),
  height: pct(l.h)
})

const ACTION_ICON: Record<SmartAction['kind'], LucideIcon> = {
  url: Link,
  email: Mail,
  phone: Phone,
  code: Hash,
  color: Palette
}
const ACTION_VERB: Record<SmartAction['kind'], string> = {
  url: 'Open link',
  email: 'Copy email',
  phone: 'Copy phone number',
  code: 'Copy code',
  color: 'Copy color'
}

interface Rect {
  x: number
  y: number
  w: number
  h: number
}

const overlaps = (a: Rect, b: OcrLine): boolean =>
  a.x < b.x + b.w && a.x + a.w > b.x && a.y < b.y + b.h && a.y + a.h > b.y

export default function Detail(props: {
  active: boolean
  hits: SearchHit[]
  index: number
  onIndex(i: number): void
  onClose(): void
  actions: Actions
  /** Visual search is on, so "Find similar" can work. */
  semantic: boolean
  query: string
}): React.JSX.Element {
  const { active, hits, index, onIndex, onClose, actions, semantic, query } = props
  const hit = hits[index]
  const shot = hit.shot
  const [loaded, setLoaded] = useState<ShotDetail | null>(null)
  const [picked, setPicked] = useState<Set<number>>(() => new Set())
  const [cropping, setCropping] = useState(false)
  const [croppingBusy, setCroppingBusy] = useState(false)
  const [crop, setCrop] = useState<Rect | null>(null)
  const [loadError, setLoadError] = useState('')
  const [band, setBand] = useState<Rect | null>(null)
  const [cursor, setCursor] = useState(0)
  const frame = useRef<HTMLDivElement>(null)
  const root = useRef<HTMLDivElement>(null)
  const textList = useRef<HTMLDivElement>(null)
  const drag = useRef<{
    x: number
    y: number
    add: boolean
    moved: boolean
    base: Set<number>
  } | null>(null)
  const d = loaded?.id === shot.id ? loaded : null
  const lines = d?.lines ?? []
  // Unread images have no size yet: take it from the image once it loads.
  const [natural, setNatural] = useState<{ id: number; a: number } | null>(null)
  const aspect =
    shot.width && shot.height
      ? shot.width / shot.height
      : natural?.id === shot.id
        ? natural.a
        : 16 / 10

  useEffect(() => root.current?.focus(), [])

  // Keep the first picked line in view in the side panel.
  useEffect(() => {
    textList.current?.querySelector('[data-picked]')?.scrollIntoView({ block: 'nearest' })
  }, [picked])

  useEffect(() => {
    let live = true
    api
      .getShot(shot.id)
      .then((r) => {
        if (live) {
          setLoaded(r)
          setLoadError('')
          if (!r) setLoadError('This image is no longer in the library.')
        }
      })
      .catch((e) => live && setLoadError(String(e)))
    return () => {
      live = false
    }
  }, [shot.id])

  const pickedText = (): string =>
    [...picked]
      .sort((a, b) => a - b)
      .map((i) => lines[i].t)
      .join('\n')

  const copy = async (): Promise<void> => {
    const text = picked.size ? pickedText() : (d?.text ?? '')
    if (!text) return actions.say('No text found in this image')
    await api.copyText(text)
    actions.say(picked.size ? `Copied ${plural(picked.size, 'line')}` : 'Copied text')
  }

  const go = (i: number): void => {
    setPicked(new Set())
    setLoadError('')
    setCrop(null)
    setCropping(false)
    setCursor(0)
    onIndex(Math.max(0, Math.min(hits.length - 1, i)))
  }

  const runAction = async (a: SmartAction): Promise<void> => {
    if (a.kind === 'url') {
      await api.openExternal(/^https?:\/\//i.test(a.value) ? a.value : `https://${a.value}`)
      return
    }
    await api.copyText(a.value)
    actions.say(`Copied ${a.value}`)
  }

  // ------------------------------------------------------------ keyboard

  const onKey = (e: KeyboardEvent): void => {
    if ((e.target as Element)?.closest('input, textarea, select')) return
    const mod = isMod(e)
    const lower = e.key.toLowerCase()
    const nativeSelection = !!window.getSelection()?.toString()
    if (e.key === 'Escape') {
      e.preventDefault()
      if (picked.size) setPicked(new Set())
      else onClose()
    } else if ((e.key === 'ArrowLeft' || e.key === 'ArrowRight') && !mod && !e.altKey) {
      e.preventDefault()
      go(index + (e.key === 'ArrowLeft' ? -1 : 1))
    } else if (mod && lower === 'c' && !(nativeSelection && !e.shiftKey)) {
      e.preventDefault()
      if (e.shiftKey) actions.copyImage(shot.id)
      else copy()
    } else if (mod && lower === 'a' && !e.shiftKey) {
      e.preventDefault()
      setPicked(new Set(lines.map((_, i) => i)))
    } else if (mod && lower === 'f' && e.shiftKey && semantic) {
      e.preventDefault()
      actions.similar(shot)
    } else if (mod && lower === 'p' && !e.shiftKey) {
      e.preventDefault()
      actions.pin(shot)
    } else if (mod && !e.repeat && (e.key === 'Delete' || e.key === 'Backspace')) {
      e.preventDefault()
      actions.trash([shot])
    }
  }
  const keyRef = useRef(onKey)
  useEffect(() => {
    keyRef.current = onKey
  })
  useEffect(() => {
    if (!active) return
    const h = (e: KeyboardEvent): void => keyRef.current(e)
    window.addEventListener('keydown', h)
    return () => window.removeEventListener('keydown', h)
  }, [active])

  // ------------------------------------------------------------ line picking

  const point = (e: React.PointerEvent): { x: number; y: number } => {
    const r = frame.current!.getBoundingClientRect()
    return {
      x: Math.max(0, Math.min(1, (e.clientX - r.left) / r.width)),
      y: Math.max(0, Math.min(1, (e.clientY - r.top) / r.height))
    }
  }

  const onDown = (e: React.PointerEvent): void => {
    if (e.button !== 0) return
    e.currentTarget.setPointerCapture(e.pointerId)
    window.getSelection()?.removeAllRanges()
    drag.current = { ...point(e), add: isMod(e) || e.shiftKey, moved: false, base: picked }
  }

  const onMove = (e: React.PointerEvent): void => {
    const g = drag.current
    if (!g) return
    const p = point(e)
    if (!g.moved && Math.hypot(p.x - g.x, p.y - g.y) < 0.008) return
    g.moved = true
    const r = {
      x: Math.min(g.x, p.x),
      y: Math.min(g.y, p.y),
      w: Math.abs(p.x - g.x),
      h: Math.abs(p.y - g.y)
    }
    setBand(r)
    if (cropping) {
      setCrop(r)
      return
    }
    const inside = lines.flatMap((l, i) => (overlaps(r, l) ? [i] : []))
    setPicked(new Set(g.add ? [...g.base, ...inside] : inside))
  }

  const onUp = (e: React.PointerEvent): void => {
    const g = drag.current
    drag.current = null
    setBand(null)
    if (cropping || !g || g.moved) return
    const p = point(e)
    const i = lines.findIndex((l) =>
      overlaps({ x: p.x - 0.004, y: p.y - 0.004, w: 0.008, h: 0.008 }, l)
    )
    if (i < 0) return setPicked(g.add ? picked : new Set())
    toggle(i, g.add)
  }

  const toggle = (i: number, add: boolean): void =>
    setPicked((prev) => {
      if (!add) return prev.size === 1 && prev.has(i) ? new Set() : new Set([i])
      const next = new Set(prev)
      if (next.has(i)) next.delete(i)
      else next.add(i)
      return next
    })

  const searchCrop = async (): Promise<void> => {
    if (!crop || crop.w <= 0 || crop.h <= 0) return
    setCroppingBusy(true)
    try {
      const image = frame.current?.querySelector('img')
      if (!image?.naturalWidth) throw new Error('Wait for the original image to load.')
      const canvas = document.createElement('canvas')
      canvas.width = Math.max(1, Math.round(crop.w * image.naturalWidth))
      canvas.height = Math.max(1, Math.round(crop.h * image.naturalHeight))
      const context = canvas.getContext('2d')
      if (!context) throw new Error('Could not prepare this crop.')
      context.drawImage(
        image,
        crop.x * image.naturalWidth,
        crop.y * image.naturalHeight,
        crop.w * image.naturalWidth,
        crop.h * image.naturalHeight,
        0,
        0,
        canvas.width,
        canvas.height
      )
      const blob = await new Promise<Blob>((resolve, reject) =>
        canvas.toBlob(
          (b) => (b ? resolve(b) : reject(new Error('Could not encode crop.'))),
          'image/png'
        )
      )
      actions.crop(new Uint8Array(await blob.arrayBuffer()), `Crop of ${shot.name}`)
    } catch (e) {
      actions.say(`Couldn’t search crop: ${String(e)}`)
    } finally {
      setCroppingBusy(false)
    }
  }

  // ------------------------------------------------------------ render

  const copyLabel = picked.size ? `Copy ${plural(picked.size, 'line')}` : 'Copy text'
  const rows: {
    icon: LucideIcon
    label: string
    combo?: string
    run(): void
    danger?: boolean
    disabled?: boolean
  }[] = [
    { icon: Copy, label: copyLabel, combo: 'Mod+C', run: copy, disabled: !d?.text },
    {
      icon: ImageIcon,
      label: 'Copy image',
      combo: 'Mod+Shift+C',
      run: () => actions.copyImage(shot.id)
    },
    { icon: ExternalLink, label: 'Open', run: () => api.open(shot.id) },
    { icon: Download, label: 'Export original and metadata…', run: () => actions.export([shot]) },
    ...(query.trim()
      ? [
          {
            icon: ThumbsDown,
            label: 'Not relevant to this search',
            run: () => actions.reject(shot)
          }
        ]
      : []),
    ...(semantic
      ? [
          {
            icon: Crop,
            label: cropping ? 'Cancel crop' : 'Search part of this image',
            run: () => {
              setCropping(!cropping)
              setCrop(null)
              setPicked(new Set())
            }
          }
        ]
      : []),
    {
      icon: FolderOpen,
      label: isMac() ? 'Show in Finder' : 'Show in folder',
      run: () => api.reveal(shot.id)
    },
    ...(semantic
      ? [
          {
            icon: ScanSearch,
            label: 'Find similar',
            combo: 'Mod+Shift+F',
            run: () => actions.similar(shot)
          }
        ]
      : []),
    {
      icon: shot.pinned ? PinOff : Pin,
      label: shot.pinned ? 'Unpin' : 'Pin',
      combo: 'Mod+P',
      run: () => actions.pin(shot)
    },
    {
      icon: Trash2,
      // A folded burst goes as a whole.
      label:
        shot.groupSize > 1
          ? `Move ${num(shot.groupSize)} shots to ${trashName()}`
          : `Move to ${trashName()}`,
      combo: trashKey(),
      run: () => actions.trash([shot]),
      danger: true
    }
  ]

  return (
    <div
      className="detail"
      role="dialog"
      aria-modal
      aria-label={shot.name}
      ref={root}
      tabIndex={-1}
    >
      <header className="bar">
        <button className="btn ghost" onClick={onClose}>
          <ArrowLeft size={15} strokeWidth={1.75} aria-hidden />
          Results
        </button>
        <span className="bar-title" />
        <span className="detail-pos">
          {num(index + 1)} of {num(hits.length)}
        </span>
        <button
          className="icon-btn"
          aria-label="Previous image"
          title="Previous"
          disabled={index === 0}
          onClick={() => go(index - 1)}
        >
          <ChevronLeft size={16} strokeWidth={1.75} />
        </button>
        <button
          className="icon-btn"
          aria-label="Next image"
          title="Next"
          disabled={index === hits.length - 1}
          onClick={() => go(index + 1)}
        >
          <ChevronRight size={16} strokeWidth={1.75} />
        </button>
      </header>

      <div className="detail-body">
        <div className="stage">
          {cropping && (
            <div className="crop-tools">
              <p>Drag over the image, or set the crop as percentages.</p>
              <div className="crop-inputs">
                {(['x', 'y', 'w', 'h'] as const).map((key) => (
                  <label key={key}>
                    {{ x: 'Left', y: 'Top', w: 'Width', h: 'Height' }[key]}
                    <input
                      type="number"
                      min={0}
                      max={100}
                      value={Math.round(
                        (crop?.[key] ?? (key === 'w' || key === 'h' ? 1 : 0)) * 100
                      )}
                      onChange={(e) =>
                        setCrop((prev) => {
                          const next = {
                            ...(prev ?? { x: 0, y: 0, w: 1, h: 1 }),
                            [key]: Math.max(0, Math.min(1, Number(e.target.value) / 100))
                          }
                          next.w = Math.min(next.w, 1 - next.x)
                          next.h = Math.min(next.h, 1 - next.y)
                          return next
                        })
                      }
                    />
                  </label>
                ))}
              </div>
              <button
                className="btn"
                disabled={!crop?.w || !crop?.h || croppingBusy}
                onClick={searchCrop}
              >
                {croppingBusy ? 'Preparing crop…' : 'Search this crop'}
              </button>
            </div>
          )}
          <div
            className="frame"
            ref={frame}
            style={{ '--a': aspect } as CSSProperties}
            onPointerDown={onDown}
            onPointerMove={onMove}
            onPointerUp={onUp}
            onPointerCancel={() => {
              drag.current = null
              setBand(null)
            }}
          >
            {/* A new element per image, with the cached thumbnail underneath, so stepping with
                arrows never shows the previous image stretched while the original decodes. */}
            <img
              key={shot.id}
              src={shot.src}
              crossOrigin="anonymous"
              alt=""
              draggable={false}
              style={{ backgroundImage: `url("${shot.thumb}")` }}
              onLoad={(e) => {
                e.currentTarget.style.backgroundImage = 'none' // transparent PNGs show no ghost
                const { naturalWidth: w, naturalHeight: h } = e.currentTarget
                if (!shot.width && w && h) setNatural({ id: shot.id, a: w / h })
              }}
            />
            {!cropping &&
              hit.highlights.map((h, i) => (
                <span key={`h${i}`} className="mark" style={box(h)} aria-hidden />
              ))}
            {!cropping &&
              lines.map((l, i) => (
                <span
                  key={i}
                  className="ocr-line"
                  data-picked={picked.has(i) || undefined}
                  style={box(l)}
                  title={l.t}
                  aria-hidden
                />
              ))}
            {cropping && crop && <span className="band crop-band" style={box(crop)} aria-hidden />}
            {band && <span className="band" style={box(band)} aria-hidden />}
          </div>
        </div>

        <aside className="panel" aria-label="Image details">
          <section
            className="panel-head"
            draggable
            title="Drag into another app"
            onDragStart={(e) => {
              e.preventDefault()
              api.startDrag(shot.id, shot.path)
            }}
          >
            <img className="proxy" src={shot.thumb} alt="" draggable={false} />
            <div className="panel-head-text">
              <h2 className="panel-name">{shot.name}</h2>
              <p className="muted">{longDate(shot.mtime)}</p>
            </div>
          </section>

          <section className="menu" aria-label="Actions">
            {rows.map((r) => (
              <button
                key={r.label}
                className="menu-row"
                data-danger={r.danger || undefined}
                disabled={r.disabled}
                onClick={() => {
                  void Promise.resolve()
                    .then(r.run)
                    .catch((e) => actions.say(`Couldn’t complete action: ${String(e)}`))
                }}
              >
                <r.icon size={15} strokeWidth={1.75} aria-hidden />
                <span>{r.label}</span>
                {r.combo && <Kbd combo={r.combo} />}
              </button>
            ))}
          </section>

          {loadError && (
            <p role="alert" className="error">
              {loadError}
            </p>
          )}
          {!!hit.evidence?.length && (
            <section>
              <h3>Why this result</h3>
              <p className="muted">{hit.evidence.join(' · ')}</p>
            </section>
          )}
          {d && (
            <MetadataForm
              key={d.id}
              detail={d}
              onSaved={(metadata) => setLoaded({ ...d, metadata })}
            />
          )}
          {/* Below the menu: it arrives a moment later and must not push the buttons down. */}
          {!!d?.actions.length && (
            <section>
              <h3>Found in this image</h3>
              <div className="smart">
                {d.actions.map((a) => {
                  const Icon = ACTION_ICON[a.kind]
                  return (
                    <button
                      key={a.kind + a.value}
                      className="smart-chip"
                      onClick={() => void runAction(a).catch((e) => actions.say(String(e)))}
                      title={`${ACTION_VERB[a.kind]}: ${a.value}`}
                    >
                      {a.kind === 'color' ? (
                        <span className="swatch" style={{ background: a.value }} aria-hidden />
                      ) : (
                        <Icon size={13} strokeWidth={1.75} aria-hidden />
                      )}
                      <span className="smart-value">{a.value}</span>
                    </button>
                  )
                })}
              </div>
            </section>
          )}

          <section>
            <h3>
              Text
              {lines.length > 0 && (
                <span className="h3-note">
                  {picked.size
                    ? `${num(picked.size)} of ${num(lines.length)} selected`
                    : 'Drag over the image to select'}
                </span>
              )}
            </h3>
            {d && !lines.length && (
              <p className="muted">
                {d.indexed === false
                  ? 'Not read yet. Its text shows up here once Magpie reads it.'
                  : 'No text found.'}
              </p>
            )}
            {/* Keyboard path for picking lines: Up/Down move, Space picks. */}
            <div
              className="ocr-text selectable"
              ref={textList}
              role="listbox"
              aria-label="Text lines"
              aria-multiselectable
              tabIndex={lines.length ? 0 : -1}
              aria-activedescendant={lines.length ? `line-${cursor}` : undefined}
              onKeyDown={(e) => {
                if (e.key === 'ArrowDown' || e.key === 'ArrowUp') {
                  e.preventDefault()
                  const next = Math.max(
                    0,
                    Math.min(lines.length - 1, cursor + (e.key === 'ArrowDown' ? 1 : -1))
                  )
                  setCursor(next)
                  document.getElementById(`line-${next}`)?.scrollIntoView({ block: 'nearest' })
                } else if (e.key === ' ' && lines.length) {
                  e.preventDefault()
                  toggle(cursor, true)
                }
              }}
            >
              {lines.map((l, i) => (
                <p
                  key={i}
                  id={`line-${i}`}
                  role="option"
                  aria-selected={picked.has(i)}
                  data-cursor={i === cursor || undefined}
                  data-picked={picked.has(i) || undefined}
                  onClick={(e) =>
                    window.getSelection()?.isCollapsed && toggle(i, isMod(e) || e.shiftKey)
                  }
                >
                  {l.t}
                </p>
              ))}
            </div>
          </section>

          <section>
            <h3>Details</h3>
            <dl className="meta selectable">
              <dt>Folder</dt>
              <dd title={shot.folder}>{basename(shot.folder)}</dd>
              <dt>Size</dt>
              <dd>
                {shot.width > 0 && `${num(shot.width)} × ${num(shot.height)} · `}
                {bytes(shot.size)}
              </dd>
              {shot.groupSize > 1 && (
                <>
                  <dt>Burst</dt>
                  <dd>{plural(shot.groupSize, 'shot')}</dd>
                </>
              )}
            </dl>
          </section>
        </aside>
      </div>
    </div>
  )
}

function MetadataForm({
  detail,
  onSaved
}: {
  detail: ShotDetail
  onSaved(metadata: ShotDetail['metadata']): void
}): React.JSX.Element {
  const [message, setMessage] = useState('')
  const [saving, setSaving] = useState(false)
  const metadata = detail.metadata ?? { note: '', tags: [], collections: [], sourceUrl: '' }
  return (
    <section>
      <h3>Your context</h3>
      <form
        className="metadata-form"
        onSubmit={async (e) => {
          e.preventDefault()
          const form = new FormData(e.currentTarget)
          const list = (name: string): string[] => [
            ...new Set(
              String(form.get(name) ?? '')
                .split(',')
                .map((v) => v.trim())
                .filter(Boolean)
            )
          ]
          const sourceUrl = String(form.get('sourceUrl') ?? '').trim()
          if (sourceUrl && !/^https?:\/\//i.test(sourceUrl))
            return setMessage('Use a full http or https source URL.')
          setSaving(true)
          try {
            onSaved(
              await api.updateMetadata(detail.id, {
                note: String(form.get('note') ?? ''),
                tags: list('tags'),
                collections: list('collections'),
                sourceUrl
              })
            )
            setMessage('Saved on this device')
          } catch (e) {
            setMessage(`Couldn’t save: ${String(e)}`)
          } finally {
            setSaving(false)
          }
        }}
      >
        <label>
          Note
          <textarea
            name="note"
            maxLength={20000}
            defaultValue={metadata.note}
            rows={3}
            placeholder="What you want to remember"
          />
        </label>
        <label>
          Tags
          <input
            name="tags"
            maxLength={2000}
            defaultValue={metadata.tags.join(', ')}
            placeholder="Comma-separated tags"
          />
        </label>
        <label>
          Collections
          <input
            name="collections"
            maxLength={2000}
            defaultValue={metadata.collections.join(', ')}
            placeholder="Comma-separated collections"
          />
        </label>
        <label>
          Source URL
          <input
            name="sourceUrl"
            type="url"
            maxLength={4096}
            defaultValue={metadata.sourceUrl}
            placeholder="https://…"
          />
        </label>
        <p className="muted">
          Source links are added by you. Search using tag:work or collection:ideas.
        </p>
        <div className="btn-row">
          <button className="btn small" disabled={saving}>
            {saving ? 'Saving…' : 'Save context'}
          </button>
          {metadata.sourceUrl && (
            <button
              type="button"
              className="btn ghost small"
              onClick={() =>
                void api.openExternal(metadata.sourceUrl).catch((e) => setMessage(String(e)))
              }
            >
              Open source
            </button>
          )}
        </div>
        <p role="status" className="muted">
          {message}
        </p>
      </form>
    </section>
  )
}
