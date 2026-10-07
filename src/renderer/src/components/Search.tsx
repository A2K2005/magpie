import { Fragment, useCallback, useEffect, useMemo, useRef, useState } from 'react'
import { flushSync } from 'react-dom'
import {
  ArrowDownUp,
  Check,
  CircleDashed,
  ImageIcon,
  LayoutGrid,
  Layers,
  List,
  Pin,
  ScanSearch,
  Search as SearchIcon,
  Settings2,
  Shuffle,
  X
} from 'lucide-react'
import type { IndexStatus, SearchHit, SearchResponse, Shot, SortOrder } from '../../../shared/types'
import {
  basename,
  bytes,
  dayGroup,
  isMac,
  isMod,
  longDate,
  num,
  plural,
  recall,
  remember,
  statusText,
  trashKey,
  when
} from '../lib/util'
import { Kbd, Thumb } from './ui'
import Detail from './Detail'

const api = window.api

/** Tiles added to the DOM per page; more load as the user scrolls or arrows near the end. */
const PAGE = 120

type View = 'grid' | 'list'
const SORTS: [SortOrder, string][] = [
  ['relevance', 'Relevance'],
  ['newest', 'Newest'],
  ['oldest', 'Oldest'],
  ['largest', 'Largest'],
  ['smallest', 'Smallest'],
  ['name', 'Name']
]
/** Orders that break the day-by-day grouping of the recent view. */
const FLAT_TITLE: Partial<Record<SortOrder, string>> = {
  largest: 'Largest first',
  smallest: 'Smallest first',
  name: 'By name'
}
const UNREAD_TIP = 'Not read yet. Found by name; text and visual search will include it soon.'

interface Ctx {
  similarTo?: { id: number; name: string }
  /** A dropped file (path) or pasted bytes (data). */
  image?: { path?: string; data?: Uint8Array; name: string }
  expand?: { id: number; size: number }
  shuffle?: boolean
}

interface Toast {
  id: number
  text: string
  undo?: boolean
}

interface Section {
  key: string
  title: string
  count?: number
  items: number[]
  divider?: boolean
}

/** Shared by the grid and the detail view. */
export interface Actions {
  say(text: string): void
  copyText(id: number): void
  copyImage(id: number): void
  pin(shot: Shot): void
  /** Folded bursts are expanded so every shot in them goes. */
  trash(shots: Shot[]): void
  similar(shot: Shot): void
}

const FILTERS: [string, string][] = [
  ['in:discord', 'Folder name contains “discord”'],
  ['path:work', 'Full path contains “work”'],
  ['ext:png;jpg', 'File type. Separate several with ;'],
  ['size:>1mb', 'Also <200kb, 1mb..5mb, or tiny, small, medium, large, huge'],
  ['width:>1920', 'Also height:. Same comparisons as size, without units'],
  ['date:today', 'Also yesterday, week, month or year. dm: works too'],
  ['before:aug', 'Before a month, or a date like 2026-08-01'],
  ['after:2026-08-01', 'On or after a date'],
  ['color:red', 'Mostly red. Any common color name works'],
  ['has:url', 'Contains a link. Also email, phone or code'],
  ['is:landscape', 'Also is:portrait or is:pinned'],
  ['sort:largest', 'Also newest, oldest, smallest, name or relevance'],
  ['-draft', 'Leave out images with this word'],
  ['"exact phrase"', 'Words next to each other, in order']
]

/** The query words without filters, for "Looks like …". */
const plainTerms = (q: string): string =>
  q
    .replace(/(^|\s)-?\w+:\S+/g, ' ')
    .replace(/(^|\s)-\S+/g, ' ')
    .replace(/"/g, '')
    .trim()

const escapeRe = (w: string): string => w.replace(/[.*+?^${}()|[\]\\]/g, '\\$&')

/** Splits `text` around `re` matches; odd parts are the matches. */
function Marked({ text, re }: { text: string; re: RegExp | null }): React.JSX.Element {
  if (!re) return <>{text}</>
  return <>{text.split(re).map((part, i) => (i % 2 ? <mark key={i}>{part}</mark> : part))}</>
}

function buildSections(res: SearchResponse, q: string, ctx: Ctx, sort: SortOrder): Section[] {
  const all = res.hits.map((_, i) => i)
  const of = (...m: string[]): number[] => all.filter((i) => m.includes(res.hits[i].match))
  if (ctx.expand)
    return [{ key: 'burst', title: `Burst of ${plural(all.length, 'shot')}`, items: all }]
  if (ctx.similarTo || ctx.image) {
    const words = plainTerms(q)
    const title = words ? `Similar to image and “${words}”` : 'Similar to image'
    return [{ key: 'img', title, items: all }]
  }
  const out: Section[] = []
  const recent = of('recent')
  if (ctx.shuffle) out.push({ key: 'shuffle', title: 'Rediscover', items: recent })
  else if (FLAT_TITLE[sort]) {
    if (recent.length) out.push({ key: 'sorted', title: FLAT_TITLE[sort]!, items: recent })
  } else
    for (const i of recent) {
      const title = dayGroup(res.hits[i].shot.mtime)
      const last = out[out.length - 1]
      if (last?.title === title) last.items.push(i)
      else out.push({ key: title, title, items: [i] })
    }
  const exact = of('text')
  if (exact.length) out.push({ key: 'text', title: 'Matches', count: exact.length, items: exact })
  // The words inside longer words ("cred" in "Credila"), and typos or misreads.
  const suggested = of('partial', 'near')
  if (suggested.length)
    out.push({
      key: 'suggested',
      title: exact.length
        ? 'Suggested'
        : res.corrected && res.didYouMean
          ? `Results for “${res.didYouMean}”`
          : 'Close matches',
      count: suggested.length,
      items: suggested,
      divider: exact.length > 0
    })
  const text = [...exact, ...suggested]
  const visual = of('visual', 'similar')
  if (visual.length)
    out.push({
      key: 'visual',
      title: `Looks like “${plainTerms(q) || q}”`,
      items: visual,
      divider: text.length > 0
    })
  return out
}

export default function Search(props: {
  active: boolean
  status: IndexStatus | null
  semantic: boolean
  sharpText: boolean
  onOpenSettings(): void
}): React.JSX.Element {
  const { active, status, semantic, sharpText, onOpenSettings } = props
  const [q, setQ] = useState('')
  const [ctx, setCtx] = useState<Ctx>({})
  const [sort, setSort] = useState<SortOrder>(() =>
    recall(
      'magpie.sort',
      SORTS.map(([s]) => s),
      'relevance'
    )
  )
  const [view, setView] = useState<View>(() => recall('magpie.view', ['grid', 'list'], 'grid'))
  const [res, setRes] = useState<SearchResponse | null>(null)
  const [sel, setSel] = useState(0)
  const [marked, setMarked] = useState<Set<number>>(() => new Set())
  const [detail, setDetail] = useState(false)
  const [toast, setToast] = useState<Toast | null>(null)
  const [dropping, setDropping] = useState(false)
  const [limit, setLimit] = useState(PAGE)
  const sentinel = useRef<HTMLDivElement>(null)
  const input = useRef<HTMLInputElement>(null)
  const grid = useRef<HTMLDivElement>(null)
  const seq = useRef(0)
  const selId = useRef<number | null>(null)

  const hits = useMemo(() => res?.hits ?? [], [res])
  const current = hits[sel] as SearchHit | undefined
  const hasCtx = !!(ctx.similarTo || ctx.image || ctx.expand || ctx.shuffle)

  useEffect(() => {
    selId.current = current?.shot.id ?? null
  }, [current])

  // Back from settings: pick up where the user left off.
  useEffect(() => {
    if (active) input.current?.focus()
  }, [active])

  // ------------------------------------------------------------ searching

  const run = useCallback(
    async (keep: boolean) => {
      const n = ++seq.current
      const r = await api.search({
        q,
        similarTo: ctx.similarTo?.id,
        imagePath: ctx.image?.path,
        imageData: ctx.image?.data,
        expandGroup: ctx.expand?.id,
        shuffle: ctx.shuffle,
        // Relevance is the default anyway, and means newest first when there are no words.
        sort: sort === 'relevance' || ctx.shuffle ? undefined : sort
      })
      if (n !== seq.current) return // a newer request is in flight; drop this one
      setRes(r)
      setSel((prev) => {
        if (!keep) return 0
        const i = r.hits.findIndex((h) => h.shot.id === selId.current)
        return i >= 0 ? i : Math.min(prev, Math.max(0, r.hits.length - 1))
      })
      if (!keep) {
        setMarked(new Set())
        setLimit(PAGE)
      }
    },
    [q, ctx, sort]
  )

  // Debounce typing; anything else (first paint, clearing, chips) runs right away.
  useEffect(() => {
    const t = setTimeout(() => run(false), q.trim() ? 60 : 0)
    return () => clearTimeout(t)
  }, [run, q])

  // Refresh quietly when new shots land, unless the user is busy with the results.
  useEffect(
    () =>
      api.onIndexed(() => {
        const idle =
          document.activeElement === input.current || document.activeElement === document.body
        if (active && !detail && marked.size === 0 && idle) run(true)
      }),
    [active, detail, marked, run]
  )

  useEffect(
    () =>
      api.onShown(() => {
        if (!active || detail) return
        input.current?.focus()
        input.current?.select()
      }),
    [active, detail]
  )

  useEffect(() => {
    if (!toast) return
    const t = setTimeout(() => setToast(null), toast.undo ? 8000 : 2400)
    return () => clearTimeout(t)
  }, [toast])

  // ------------------------------------------------------------ actions

  const say = useCallback(
    (text: string, undo = false) => setToast({ id: Date.now(), text, undo }),
    []
  )

  const actions: Actions = useMemo(
    () => ({
      say,
      async copyText(id) {
        const d = await api.getShot(id)
        if (!d?.text) return say('No text found in this image')
        await api.copyText(d.text)
        say('Copied text')
      },
      async copyImage(id) {
        await api.copyImage(id)
        say('Copied image')
      },
      async pin(shot) {
        await api.pin(shot.id, !shot.pinned)
        say(shot.pinned ? 'Unpinned' : 'Pinned')
        run(true)
      },
      async trash(shots) {
        if (!shots.length) return
        const ids = new Set(shots.map((s) => s.id))
        for (const s of shots) {
          if (s.groupSize < 2 || s.groupId === null) continue
          const burst = await api.search({ q: '', expandGroup: s.groupId, limit: 500 })
          burst.hits.forEach((h) => ids.add(h.shot.id))
        }
        await api.trash([...ids])
        setMarked(new Set())
        setDetail(false)
        say(`Moved ${plural(ids.size, 'image')} to Trash`, true)
        run(true)
      },
      similar(shot) {
        setDetail(false)
        setQ('')
        setCtx({ similarTo: { id: shot.id, name: shot.name } })
      }
    }),
    [say, run]
  )

  const undo = async (): Promise<void> => {
    const n = await api.undoTrash()
    say(n ? `Restored ${plural(n, 'image')}` : 'Nothing to restore')
    run(true)
  }

  const expand = (shot: Shot): void => {
    if (shot.groupSize < 2 || shot.groupId === null) return
    setCtx({ expand: { id: shot.groupId, size: shot.groupSize } })
  }

  const clearAll = (): void => {
    setQ('')
    setCtx({})
    input.current?.focus()
  }

  const removeFilter = (key: string, value: string): void => {
    // `dm:` is typed but comes back as a date filter.
    const tokens = (
      key === 'not'
        ? [`-${value}`]
        : (key === 'date' ? ['date', 'dm'] : [key]).map((k) => `${k}:${value}`)
    ).map((t) => t.toLowerCase())
    setQ((prev) =>
      prev
        .split(/\s+/)
        .filter((t) => !tokens.includes(t.toLowerCase()))
        .join(' ')
    )
    input.current?.focus()
  }

  const pickSort = (next: SortOrder, fromQuery?: string): void => {
    if (fromQuery) removeFilter('sort', fromQuery) // the menu now owns the order
    setSort(next)
    remember('magpie.sort', next)
  }

  const toggleView = (next: View = view === 'grid' ? 'list' : 'grid'): void => {
    const hadFocus = !!grid.current?.contains(document.activeElement)
    flushSync(() => setView(next))
    remember('magpie.view', next)
    // The selected result is a new element now: keep focus and scroll on it.
    if (hadFocus) tileEl(sel)?.focus()
    else tileEl(sel)?.scrollIntoView({ block: 'nearest' })
  }

  // ------------------------------------------------------------ layout

  // Image context stays while typing: words and image are sent together and blended.
  const words = plainTerms(q)
  /** Relevance only means something with words or an image to compare against. */
  const ranked = !!(words || ctx.similarTo || ctx.image)
  const typedSort = res?.filters.find((f) => f.key === 'sort')?.value
  const querySort = SORTS.find(([s]) => s === typedSort)?.[0]
  const shownSort = querySort ?? (sort === 'relevance' && !ranked ? 'newest' : sort)
  const terms = useMemo(() => {
    const t = words.toLowerCase().split(/\s+/).filter(Boolean)
    return t.length ? new RegExp(`(${t.map(escapeRe).join('|')})`, 'gi') : null
  }, [words])

  const sections = useMemo(
    () => (res ? buildSections(res, q, ctx, shownSort) : []),
    [res, q, ctx, shownSort]
  )
  const order = useMemo(() => sections.flatMap((s) => s.items), [sections])
  const rendered = useMemo(() => new Set(order.slice(0, limit)), [order, limit])

  /** Grows the window so hit `i` (and a page after it) is in the DOM. */
  const reveal = (i: number): void => {
    const pos = order.indexOf(i)
    if (pos >= limit - 24) setLimit(pos + PAGE)
  }

  useEffect(() => {
    const el = sentinel.current
    if (!el) return
    const io = new IntersectionObserver(([e]) => e.isIntersecting && setLimit((l) => l + PAGE), {
      root: grid.current,
      rootMargin: '600px'
    })
    io.observe(el)
    return () => io.disconnect()
  }, [order, limit])

  const tileEl = (i: number): HTMLElement | null =>
    grid.current?.querySelector<HTMLElement>(`[data-index="${i}"]`) ?? null

  const select = (i: number, focus: boolean): void => {
    setSel(i)
    reveal(i)
    const el = tileEl(i)
    if (focus) el?.focus()
    else el?.scrollIntoView({ block: 'nearest' })
  }

  // Keep the selected tile visible when the results change underneath it.
  useEffect(() => {
    if (!detail) tileEl(sel)?.scrollIntoView({ block: 'nearest' })
  }, [sel, res, detail])

  /** 2D movement by on-screen position, so it works across section breaks. */
  const move = (key: string): void => {
    const pos = order.indexOf(sel)
    if (key === 'ArrowLeft') return select(order[Math.max(0, pos - 1)], true)
    if (key === 'ArrowRight') return select(order[Math.min(order.length - 1, pos + 1)], true)
    const cur = tileEl(sel)?.getBoundingClientRect()
    if (!cur) return
    const down = key === 'ArrowDown'
    let best: { i: number; dy: number; dx: number } | null = null
    for (const i of order) {
      const r = tileEl(i)?.getBoundingClientRect()
      if (!r) continue
      const dy = down ? r.top - cur.bottom : cur.top - r.bottom
      if (dy < -2) continue
      const dx = Math.abs(r.left + r.width / 2 - (cur.left + cur.width / 2))
      if (!best || dy < best.dy - 4 || (Math.abs(dy - best.dy) <= 4 && dx < best.dx))
        best = { i, dy, dx }
    }
    if (best) select(best.i, true)
    else if (!down) input.current?.focus()
  }

  const markRange = (to: number): void => {
    const a = order.indexOf(sel)
    const b = order.indexOf(to)
    const [lo, hi] = a < b ? [a, b] : [b, a]
    setMarked(new Set(order.slice(lo, hi + 1).map((i) => hits[i].shot.id)))
  }

  const onTileDown = (e: React.MouseEvent, i: number): void => {
    if (e.button !== 0) return
    const id = hits[i].shot.id
    if (isMod(e)) {
      setMarked((prev) => {
        const next = new Set(prev.size || i === sel || !current ? prev : [current.shot.id])
        if (next.has(id)) next.delete(id)
        else next.add(id)
        return next
      })
    } else if (e.shiftKey) {
      e.preventDefault()
      markRange(i)
      return void tileEl(i)?.focus()
    } else setMarked(new Set())
    setSel(i)
  }

  const targets = (): Shot[] =>
    marked.size
      ? hits.filter((h) => marked.has(h.shot.id)).map((h) => h.shot)
      : current
        ? [current.shot]
        : []

  // ------------------------------------------------------------ keyboard

  const onKey = (e: KeyboardEvent): void => {
    const k = e.key
    const mod = isMod(e)
    const inInput = document.activeElement === input.current
    const lower = k.toLowerCase()
    const inputHasSelection =
      inInput && input.current!.selectionStart !== input.current!.selectionEnd
    // Buttons and the sort menu keep their own Enter, Space and arrow keys.
    const inControl = !inInput && !!(e.target as Element | null)?.closest?.('button, select')

    if (k === 'Escape') {
      e.preventDefault()
      if (marked.size) setMarked(new Set())
      else if (!inInput) input.current?.focus()
      else if (q)
        setQ('') // words first, then any image context
      else if (hasCtx) setCtx({})
      else api.hide()
      return
    }
    if (mod && e.shiftKey && lower === 'a') {
      e.preventDefault()
      setMarked(new Set(hits.map((h) => h.shot.id)))
      return
    }
    if (mod && (k === 'Delete' || k === 'Backspace')) {
      // In a non-empty search box with nothing marked, let Ctrl+Backspace delete a word.
      if (inInput && q && !marked.size) return
      e.preventDefault()
      actions.trash(targets())
      return
    }
    if (mod && !e.shiftKey && lower === 'z' && toast?.undo) {
      e.preventDefault()
      undo()
      return
    }
    if (mod && !e.shiftKey && !e.altKey && lower === 'l') {
      e.preventDefault()
      toggleView()
      return
    }
    if (inControl && (k === 'Enter' || k === ' ' || k.startsWith('Arrow'))) return
    if (!current) {
      if (inInput && k === 'Enter' && q) e.preventDefault()
      return
    }
    if (mod && lower === 'c') {
      if (inputHasSelection) return
      e.preventDefault()
      if (e.shiftKey) actions.copyImage(current.shot.id)
      else actions.copyText(current.shot.id)
      return
    }
    if (mod && !e.shiftKey && lower === 'f') {
      e.preventDefault()
      actions.similar(current.shot)
      return
    }
    if (mod && !e.shiftKey && lower === 'p') {
      e.preventDefault()
      actions.pin(current.shot)
      return
    }
    if (mod && !e.shiftKey && lower === 'e') {
      e.preventDefault()
      expand(current.shot)
      return
    }
    if (k === 'Enter' && !mod) {
      e.preventDefault()
      setDetail(true)
      return
    }
    if (k.startsWith('Arrow') && !mod && !e.altKey) {
      if (inInput) {
        if (k === 'ArrowDown') {
          e.preventDefault()
          select(sel, true)
        }
        return
      }
      e.preventDefault()
      move(k)
      return
    }
    // Typing while the grid has focus goes back to the search box.
    if (!inInput && !mod && !e.altKey && (k.length === 1 || k === 'Backspace'))
      input.current?.focus()
  }

  const keyRef = useRef(onKey)
  useEffect(() => {
    keyRef.current = onKey
  })
  useEffect(() => {
    if (!active || detail) return
    const h = (e: KeyboardEvent): void => keyRef.current(e)
    window.addEventListener('keydown', h)
    return () => window.removeEventListener('keydown', h)
  }, [active, detail])

  // ------------------------------------------------------------ drag and drop

  useEffect(() => {
    if (!active) return
    const isFile = (e: DragEvent): boolean => !!e.dataTransfer?.types.includes('Files')
    const over = (e: DragEvent): void => {
      if (!isFile(e)) return
      e.preventDefault()
      setDropping(true)
    }
    const leave = (e: DragEvent): void => {
      if (!e.relatedTarget) setDropping(false)
    }
    const drop = (e: DragEvent): void => {
      if (!isFile(e)) return
      e.preventDefault()
      setDropping(false)
      const file = e.dataTransfer!.files[0]
      if (!file?.type.startsWith('image/')) return say('Drop an image file, like a PNG or JPG')
      setDetail(false)
      setQ('')
      // Web views without file paths (Tauri) search by the image bytes instead.
      const path = api.pathForFile(file)
      if (path) setCtx({ image: { path, name: file.name } })
      else
        void file
          .arrayBuffer()
          .then((b) => setCtx({ image: { data: new Uint8Array(b), name: file.name } }))
    }
    window.addEventListener('dragover', over)
    window.addEventListener('dragleave', leave)
    window.addEventListener('drop', drop)
    return () => {
      window.removeEventListener('dragover', over)
      window.removeEventListener('dragleave', leave)
      window.removeEventListener('drop', drop)
    }
  }, [active, say])

  // ------------------------------------------------------------ paste an image

  useEffect(() => {
    if (!active || detail) return
    const onPaste = (e: ClipboardEvent): void => {
      // Text typed into the box wins: only take over when it is empty or not focused.
      if (document.activeElement === input.current && q) return
      const item = [...(e.clipboardData?.items ?? [])].find(
        (i) => i.kind === 'file' && i.type.startsWith('image/')
      )
      const file = item?.getAsFile()
      if (!file) return
      e.preventDefault()
      file.arrayBuffer().then((buf) => {
        setQ('')
        setCtx({ image: { data: new Uint8Array(buf), name: 'pasted image' } })
        input.current?.focus()
      })
    }
    window.addEventListener('paste', onPaste)
    return () => window.removeEventListener('paste', onPaste)
  }, [active, detail, q])

  // ------------------------------------------------------------ render

  const imageLabel = ctx.similarTo
    ? `Similar to ${ctx.similarTo.name}`
    : ctx.image?.path
      ? `Image: ${ctx.image.name}`
      : 'Pasted image'
  const chips = [
    ...(ctx.similarTo || ctx.image
      ? [
          {
            id: 'img',
            label: words ? `Image + “${words}”` : imageLabel,
            icon: ctx.similarTo ? ScanSearch : ImageIcon
          }
        ]
      : []),
    ...(ctx.expand ? [{ id: 'burst', label: `Burst of ${ctx.expand.size}`, icon: Layers }] : [])
  ]
  const filters = res?.filters ?? []
  const empty = res && hits.length === 0
  const searching = !!(q.trim() || hasCtx)

  return (
    <div className="search" inert={!active || undefined}>
      <div className="search-main" inert={detail || undefined}>
        <header className="search-bar">
          <div className="search-field">
            <SearchIcon className="search-icon" size={18} strokeWidth={1.75} aria-hidden />
            <input
              ref={input}
              className="search-input"
              value={q}
              onChange={(e) => {
                setQ(e.target.value)
                if (ctx.shuffle) setCtx({})
              }}
              placeholder={
                ctx.similarTo || ctx.image ? 'Narrow with words or filters' : 'Search images'
              }
              aria-label="Search images"
              role="combobox"
              aria-expanded={hits.length > 0}
              aria-controls="results"
              aria-activedescendant={current ? `hit-${sel}` : undefined}
              autoFocus
              spellCheck={false}
            />
            <label className="sort" title="Sort results">
              <ArrowDownUp size={14} strokeWidth={1.75} aria-hidden />
              <select
                aria-label="Sort results"
                value={shownSort}
                onChange={(e) => pickSort(e.target.value as SortOrder, typedSort)}
              >
                {SORTS.filter(([s]) => ranked || s !== 'relevance' || shownSort === s).map(
                  ([s, label]) => (
                    <option key={s} value={s}>
                      {label}
                    </option>
                  )
                )}
              </select>
            </label>
            <div className="view-toggle" role="group" aria-label="Layout">
              {(
                [
                  ['grid', 'Grid', LayoutGrid],
                  ['list', 'List', List]
                ] as const
              ).map(([v, label, Icon]) => (
                <button
                  key={v}
                  aria-label={label}
                  aria-pressed={view === v}
                  title={`${label} (${isMac() ? '⌘' : 'Ctrl+'}L)`}
                  onClick={() => toggleView(v)}
                >
                  <Icon size={15} strokeWidth={1.75} aria-hidden />
                </button>
              ))}
            </div>
            <button
              className="icon-btn"
              onClick={onOpenSettings}
              aria-label="Settings"
              title="Settings"
            >
              <Settings2 size={16} strokeWidth={1.75} />
            </button>
          </div>
          {(chips.length > 0 || filters.length > 0) && (
            <div className="chips" aria-label="Active filters">
              {chips.map((c) => (
                <span className="chip" key={c.id}>
                  <c.icon size={13} strokeWidth={1.75} aria-hidden />
                  <span className="chip-label">{c.label}</span>
                  <button
                    className="chip-x"
                    aria-label={c.id === 'img' ? 'Remove image' : `Remove ${c.label}`}
                    onClick={() => setCtx({})}
                  >
                    <X size={12} strokeWidth={2} />
                  </button>
                </span>
              ))}
              {filters.map((f) => (
                <span className="chip" key={f.key + f.value}>
                  <span className="chip-label">{f.label}</span>
                  <button
                    className="chip-x"
                    aria-label={`Remove filter ${f.label}`}
                    onClick={() => removeFilter(f.key, f.value)}
                  >
                    <X size={12} strokeWidth={2} />
                  </button>
                </span>
              ))}
            </div>
          )}
        </header>

        <main className="results" ref={grid}>
          {empty && searching && (
            <div className="empty">
              <h2>No images match{q.trim() ? ` “${q.trim()}”` : ''}</h2>
              <p>Check the spelling, try fewer words, or narrow down with filters:</p>
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
              {(res.semantic === 'downloading' || res.semantic === 'loading') && (
                <p className="section-note">Visual matches are warming up…</p>
              )}
              <div className="empty-actions">
                <button className="btn" onClick={clearAll}>
                  Clear search
                </button>
                {!semantic && (
                  <button className="btn ghost" onClick={onOpenSettings}>
                    Turn on search by what images show
                  </button>
                )}
              </div>
            </div>
          )}
          {empty && !searching && (
            <div className="empty">
              <h2>No images yet</h2>
              <p>
                {status?.state === 'indexing' || status?.state === 'scanning'
                  ? 'Magpie is reading your folders. Shots show up here as they’re indexed.'
                  : 'Add the folders where your images are saved, and they’ll show up here.'}
              </p>
              <div className="empty-actions">
                <button className="btn" onClick={onOpenSettings}>
                  Choose folders
                </button>
              </div>
            </div>
          )}
          {res?.didYouMean && (
            <p className="did-you-mean">
              {res.corrected ? (
                <>
                  Showing results for <strong>{res.didYouMean}</strong>
                  <span className="dot">·</span>
                  <span className="muted">No images contain “{q.trim()}”</span>
                </>
              ) : (
                <>
                  Did you mean{' '}
                  <button type="button" className="link" onClick={() => setQ(res.didYouMean!)}>
                    {res.didYouMean}
                  </button>
                  ?
                </>
              )}
            </p>
          )}
          {!empty && (
            <div
              id="results"
              role="listbox"
              aria-label="Images"
              aria-multiselectable
              className="sections"
            >
              {sections.map((s, si) => {
                const items = s.items.filter((i) => rendered.has(i))
                if (!items.length) return null
                return (
                  <div
                    role="group"
                    aria-labelledby={`sec-${s.key}`}
                    key={s.key}
                    data-divider={s.divider || undefined}
                  >
                    <div className="section-head">
                      <h2 id={`sec-${s.key}`}>{s.title}</h2>
                      {s.count !== undefined && (
                        <span className="section-count">{num(s.count)}</span>
                      )}
                      {si === 0 && !searching && (
                        <button
                          className="btn ghost small push"
                          onClick={() => setCtx({ shuffle: true })}
                        >
                          <Shuffle size={13} strokeWidth={1.75} aria-hidden />
                          {ctx.shuffle ? 'Shuffle again' : 'Shuffle'}
                        </button>
                      )}
                      {si === 0 && ctx.shuffle && (
                        <button className="btn ghost small" onClick={() => setCtx({})}>
                          Back to recent
                        </button>
                      )}
                    </div>
                    <div className={view === 'list' ? 'hit-rows' : 'grid'}>
                      {items.map((i) => {
                        const Item = view === 'list' ? Row : Tile
                        return (
                          <Item
                            key={hits[i].shot.id}
                            hit={hits[i]}
                            index={i}
                            selected={i === sel}
                            marked={marked.has(hits[i].shot.id)}
                            terms={terms}
                            onMouseDown={onTileDown}
                            onOpen={() => {
                              setSel(i)
                              setDetail(true)
                            }}
                            onExpand={() => expand(hits[i].shot)}
                          />
                        )
                      })}
                    </div>
                    {(s.key === 'suggested' ||
                      (s.key === 'text' && !sections.some((x) => x.key === 'suggested'))) &&
                      (res?.semantic === 'loading' || res?.semantic === 'downloading') && (
                        <p className="section-note">Visual matches are warming up…</p>
                      )}
                  </div>
                )
              })}
              {order.length > limit && <div ref={sentinel} className="sentinel" />}
            </div>
          )}
        </main>

        <footer className="footer">
          <span className="footer-status" aria-live="off">
            {statusText(status, semantic, sharpText)}
            {res && searching && !empty && ` · ${res.tookMs} ms`}
          </span>
          <span className="footer-hints">
            {marked.size > 0 ? (
              <>
                <span className="hint-strong">{num(marked.size)} selected</span>
                <span className="hint">
                  <Kbd combo={trashKey()} /> Move to Trash
                </span>
                <span className="hint">
                  <Kbd combo="Esc" /> Clear
                </span>
              </>
            ) : current ? (
              <>
                <span className="hint">
                  <Kbd combo="Enter" /> Open
                </span>
                <span className="hint">
                  <Kbd combo="Mod+C" /> Copy text
                </span>
                {current.shot.groupSize > 1 ? (
                  <span className="hint">
                    <Kbd combo="Mod+E" /> Show burst
                  </span>
                ) : (
                  <span className="hint">
                    <Kbd combo="Mod+F" /> Similar
                  </span>
                )}
              </>
            ) : null}
          </span>
        </footer>
      </div>

      <div className="toast-region" role="status" aria-live="polite">
        {toast && (
          <div className="toast" key={toast.id}>
            <span>{toast.text}</span>
            {toast.undo && (
              <button className="btn ghost small" onClick={undo}>
                Undo <Kbd combo="Mod+Z" />
              </button>
            )}
          </div>
        )}
      </div>

      {dropping && (
        <div className="drop" aria-hidden>
          <div className="drop-inner">
            <ImageIcon size={22} strokeWidth={1.5} />
            <span>Drop an image to find images that look like it</span>
          </div>
        </div>
      )}

      {detail && current && (
        <Detail
          active={active}
          hits={hits}
          index={sel}
          onIndex={(i) => {
            setSel(i)
            reveal(i)
          }}
          onClose={() => {
            flushSync(() => setDetail(false))
            tileEl(sel)?.focus()
          }}
          actions={actions}
        />
      )}
    </div>
  )
}

interface ItemProps {
  hit: SearchHit
  index: number
  selected: boolean
  marked: boolean
  /** Query words, for emphasis in the list view. */
  terms: RegExp | null
  onMouseDown(e: React.MouseEvent, i: number): void
  onOpen(): void
  onExpand(): void
}

/** What a grid tile and a list row share: option semantics, selection, open, drag. */
function itemAttrs(props: ItemProps, className: string): React.HTMLAttributes<HTMLDivElement> {
  const { hit, index, selected, marked } = props
  return {
    role: 'option',
    id: `hit-${index}`,
    'data-index': index,
    className,
    'aria-selected': marked || selected,
    'data-current': selected || undefined,
    'data-marked': marked || undefined,
    tabIndex: selected ? 0 : -1,
    onMouseDown: (e) => props.onMouseDown(e, index),
    onDoubleClick: props.onOpen,
    draggable: true,
    onDragStart: (e) => {
      e.preventDefault()
      api.startDrag(hit.shot.id, hit.shot.path)
    }
  } as React.HTMLAttributes<HTMLDivElement>
}

const Unread = (): React.JSX.Element => (
  <CircleDashed className="unread" size={11} strokeWidth={2} aria-label={UNREAD_TIP}>
    <title>{UNREAD_TIP}</title>
  </CircleDashed>
)

function Tile(props: ItemProps): React.JSX.Element {
  const { hit, marked } = props
  const { shot } = hit
  const inked = hit.match === 'text' || hit.match === 'partial' || hit.match === 'near'
  const title = hit.snippet ?? shot.name
  return (
    <div {...itemAttrs(props, 'tile')}>
      <div className="tile-media" data-stack={shot.groupSize > 1 || undefined}>
        <Thumb shot={shot} highlights={hit.highlights} inked={inked} />
        {marked && (
          <span className="tile-check" aria-hidden>
            <Check size={12} strokeWidth={2.5} />
          </span>
        )}
        {shot.groupSize > 1 && (
          <button
            className="tile-count"
            tabIndex={-1}
            aria-label={`Show all ${shot.groupSize} shots in this burst`}
            onMouseDown={(e) => e.stopPropagation()}
            onClick={props.onExpand}
          >
            <Layers size={12} strokeWidth={2} aria-hidden />
            {shot.groupSize}
          </button>
        )}
      </div>
      <div className="tile-caption">
        <span className="tile-title" title={title}>
          {title}
        </span>
        <span className="tile-meta">
          {hit.match === 'near' && <span className="tag">Near match</span>}
          {shot.pinned && (
            <Pin className="tile-pin" size={11} strokeWidth={2} aria-label="Pinned" />
          )}
          {shot.indexed === false && <Unread />}
          <span className="tile-when">{when(shot.mtime)}</span>
          <span className="tile-folder" title={shot.folder}>
            {basename(shot.folder)}
          </span>
        </span>
      </div>
    </div>
  )
}

/** Dense list row: thumbnail, name over folder, then dimensions, size and date. */
function Row(props: ItemProps): React.JSX.Element {
  const { hit, marked, terms } = props
  const { shot } = hit
  const cut = Math.max(shot.folder.lastIndexOf('\\'), shot.folder.lastIndexOf('/')) + 1
  const unread = shot.indexed === false
  return (
    <div {...itemAttrs(props, 'hit-row')}>
      <span className="hit-thumb">
        <img src={shot.thumb} alt="" draggable={false} loading="lazy" decoding="async" />
        {marked && (
          <span className="tile-check" aria-hidden>
            <Check size={12} strokeWidth={2.5} />
          </span>
        )}
      </span>
      <span className="hit-text">
        <span
          className="hit-name"
          title={hit.snippet ? `${shot.name}\n“${hit.snippet}”` : shot.name}
        >
          <span className="hit-name-text">
            <Marked text={shot.name} re={terms} />
          </span>
          {hit.match === 'near' && <span className="tag">Near match</span>}
          {shot.pinned && (
            <Pin className="tile-pin" size={11} strokeWidth={2} aria-label="Pinned" />
          )}
          {unread && <Unread />}
          {shot.groupSize > 1 && (
            <button
              className="hit-count"
              tabIndex={-1}
              aria-label={`Show all ${shot.groupSize} shots in this burst`}
              onMouseDown={(e) => e.stopPropagation()}
              onClick={props.onExpand}
            >
              <Layers size={11} strokeWidth={2} aria-hidden />
              {shot.groupSize}
            </button>
          )}
          {/* Why a text hit is here, when the name alone doesn't say. */}
          {hit.snippet && (
            <span className="hit-snippet">
              “<Marked text={hit.snippet} re={terms} />”
            </span>
          )}
        </span>
        {/* Middle truncation: the start of the path gives way first, the folder name stays. */}
        <span className="hit-path" title={shot.folder}>
          <span className="hit-path-head">{shot.folder.slice(0, cut)}</span>
          <span className="hit-path-tail">{shot.folder.slice(cut)}</span>
        </span>
      </span>
      <span className="hit-dims" data-unread={unread || undefined}>
        {unread ? 'Not read yet' : `${num(shot.width)} × ${num(shot.height)}`}
      </span>
      <span className="hit-size">{bytes(shot.size)}</span>
      <span className="hit-date" title={longDate(shot.mtime)}>
        {when(shot.mtime)}
      </span>
    </div>
  )
}
