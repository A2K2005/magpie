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
  Trash2
} from 'lucide-react'
import type { LucideIcon } from 'lucide-react'
import type { OcrLine, SearchHit, ShotDetail, SmartAction } from '../../../shared/types'
import { basename, bytes, isMac, isMod, longDate, num, plural, trashKey } from '../lib/util'
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
}): React.JSX.Element {
  const { active, hits, index, onIndex, onClose, actions } = props
  const hit = hits[index]
  const shot = hit.shot
  const [loaded, setLoaded] = useState<ShotDetail | null>(null)
  const [picked, setPicked] = useState<Set<number>>(() => new Set())
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
    api.getShot(shot.id).then((r) => live && setLoaded(r))
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
    } else if (mod && lower === 'f' && !e.shiftKey) {
      e.preventDefault()
      actions.similar(shot)
    } else if (mod && lower === 'p' && !e.shiftKey) {
      e.preventDefault()
      actions.pin(shot)
    } else if (mod && (e.key === 'Delete' || e.key === 'Backspace')) {
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
    return { x: (e.clientX - r.left) / r.width, y: (e.clientY - r.top) / r.height }
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
    const inside = lines.flatMap((l, i) => (overlaps(r, l) ? [i] : []))
    setPicked(new Set(g.add ? [...g.base, ...inside] : inside))
  }

  const onUp = (e: React.PointerEvent): void => {
    const g = drag.current
    drag.current = null
    setBand(null)
    if (!g || g.moved) return
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

  // ------------------------------------------------------------ render

  const copyLabel = picked.size ? `Copy ${plural(picked.size, 'line')}` : 'Copy text'
  const rows: { icon: LucideIcon; label: string; combo?: string; run(): void; danger?: boolean }[] =
    [
      { icon: Copy, label: copyLabel, combo: 'Mod+C', run: copy },
      {
        icon: ImageIcon,
        label: 'Copy image',
        combo: 'Mod+Shift+C',
        run: () => actions.copyImage(shot.id)
      },
      { icon: ExternalLink, label: 'Open', run: () => api.open(shot.id) },
      {
        icon: FolderOpen,
        label: isMac() ? 'Show in Finder' : 'Show in folder',
        run: () => api.reveal(shot.id)
      },
      { icon: ScanSearch, label: 'Find similar', combo: 'Mod+F', run: () => actions.similar(shot) },
      {
        icon: shot.pinned ? PinOff : Pin,
        label: shot.pinned ? 'Unpin' : 'Pin',
        combo: 'Mod+P',
        run: () => actions.pin(shot)
      },
      {
        icon: Trash2,
        label: 'Move to Trash',
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
            <img
              src={shot.src}
              alt=""
              draggable={false}
              onLoad={(e) => {
                const { naturalWidth: w, naturalHeight: h } = e.currentTarget
                if (!shot.width && w && h) setNatural({ id: shot.id, a: w / h })
              }}
            />
            {hit.highlights.map((h, i) => (
              <span key={`h${i}`} className="mark" style={box(h)} aria-hidden />
            ))}
            {lines.map((l, i) => (
              <span
                key={i}
                className="ocr-line"
                data-picked={picked.has(i) || undefined}
                style={box(l)}
                title={l.t}
                aria-hidden
              />
            ))}
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

          {!!d?.actions.length && (
            <section>
              <h3>Found in this shot</h3>
              <div className="smart">
                {d.actions.map((a) => {
                  const Icon = ACTION_ICON[a.kind]
                  return (
                    <button
                      key={a.kind + a.value}
                      className="smart-chip"
                      onClick={() => runAction(a)}
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

          <section className="menu" aria-label="Actions">
            {rows.map((r) => (
              <button
                key={r.label}
                className="menu-row"
                data-danger={r.danger || undefined}
                onClick={r.run}
              >
                <r.icon size={15} strokeWidth={1.75} aria-hidden />
                <span>{r.label}</span>
                {r.combo && <Kbd combo={r.combo} />}
              </button>
            ))}
          </section>

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

          {shot.colors.length > 0 && (
            <section>
              <h3>Colors</h3>
              <div className="colors">
                {shot.colors.map((c) => (
                  <button
                    key={c}
                    className="color"
                    onClick={() => runAction({ kind: 'color', value: c })}
                    title={`Copy ${c}`}
                  >
                    <span className="swatch" style={{ background: c }} aria-hidden />
                    <span>{c}</span>
                  </button>
                ))}
              </div>
            </section>
          )}
        </aside>
      </div>
    </div>
  )
}
