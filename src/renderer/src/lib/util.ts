import type { IndexStatus } from '../../../shared/types'

export const isMac = (): boolean => window.api.platform === 'darwin'

/** Cmd on macOS, Ctrl elsewhere. */
export const isMod = (e: { metaKey: boolean; ctrlKey: boolean }): boolean =>
  isMac() ? e.metaKey : e.ctrlKey

const nf = new Intl.NumberFormat()
export const num = (n: number): string => nf.format(n)
export const plural = (n: number, one: string, many = one + 's'): string =>
  `${num(n)} ${n === 1 ? one : many}`

export function bytes(n: number): string {
  const units = ['bytes', 'KB', 'MB', 'GB', 'TB']
  let i = 0
  while (n >= 1000 && i < units.length - 1) {
    n /= 1000
    i++
  }
  return `${i ? n.toFixed(n < 10 ? 1 : 0) : n} ${units[i]}`
}

export const basename = (p: string): string => p.split(/[\\/]/).filter(Boolean).pop() ?? p

const startOfDay = (t: number): number => {
  const d = new Date(t)
  return new Date(d.getFullYear(), d.getMonth(), d.getDate()).getTime()
}
const DAY = 86400_000
const fmtMonth = new Intl.DateTimeFormat(undefined, { month: 'long' })
const fmtMonthYear = new Intl.DateTimeFormat(undefined, { month: 'long', year: 'numeric' })
const fmtWeekday = new Intl.DateTimeFormat(undefined, { weekday: 'long' })
const fmtTime = new Intl.DateTimeFormat(undefined, { hour: 'numeric', minute: '2-digit' })
const fmtShort = new Intl.DateTimeFormat(undefined, { day: 'numeric', month: 'short' })
const fmtLong = new Intl.DateTimeFormat(undefined, {
  day: 'numeric',
  month: 'long',
  year: 'numeric',
  hour: 'numeric',
  minute: '2-digit'
})

/** Section label for the recent view: Today, Yesterday, This week, then month names. */
export function dayGroup(t: number, now = Date.now()): string {
  const today = startOfDay(now)
  if (t >= today) return 'Today'
  if (t >= today - DAY) return 'Yesterday'
  if (t >= today - 6 * DAY) return 'This week'
  const d = new Date(t)
  return d.getFullYear() === new Date(now).getFullYear()
    ? fmtMonth.format(d)
    : fmtMonthYear.format(d)
}

/** Compact time for captions: "2:14 PM", "Monday", "12 Sep". */
export function when(t: number, now = Date.now()): string {
  const today = startOfDay(now)
  if (t >= today) return fmtTime.format(t)
  if (t >= today - DAY) return `Yesterday, ${fmtTime.format(t)}`
  if (t >= today - 6 * DAY) return fmtWeekday.format(t)
  return fmtShort.format(t)
}

export const longDate = (t: number): string => fmtLong.format(t)

/** Footer line for the index state. */
export function statusText(s: IndexStatus | null, semantic: boolean, sharpText = false): string {
  if (!s) return ''
  const parts: string[] = []
  const read = s.ocrDone + s.errors
  const visualLeft = semantic && s.model.state === 'ready' && s.embedded + s.errors < s.total
  if (s.state === 'scanning') parts.push('Looking for images…')
  else if (s.state === 'paused') parts.push(`Paused at ${num(read)} of ${num(s.total)} read`)
  else if (read < s.total) parts.push(`Reading ${num(read)} of ${num(s.total)}`)
  else parts.push(plural(s.total, 'image'))
  if (read >= s.total && visualLeft)
    parts.push(`visual search ${num(s.embedded)} of ${num(s.total)}, finishes while you’re away`)
  if (s.model.state === 'downloading')
    parts.push(`Downloading visual model ${Math.round((s.model.progress ?? 0) * 100)}%`)
  else if (s.model.state === 'loading') parts.push('Loading visual model…')
  else if (s.model.state === 'error') parts.push('Visual search unavailable')
  const t = s.textModel
  if (sharpText && t?.state === 'downloading')
    parts.push(`Downloading text model ${Math.round((t.progress ?? 0) * 100)}%`)
  else if (sharpText && t?.state === 'error') parts.push('Sharper text unavailable')
  else if (sharpText && t?.state === 'ready' && s.sharp < s.ocrDone)
    parts.push(`Sharpening text ${num(s.sharp)} of ${num(s.ocrDone)}`)
  if (s.errors && s.state === 'idle') parts.push(`${plural(s.errors, 'file')} couldn’t be read`)
  return parts.join(' · ')
}

/** A remembered per-viewer choice, or `fallback` when storage is unavailable or holds junk. */
export function recall<T extends string>(key: string, allowed: readonly T[], fallback: T): T {
  try {
    const v = localStorage.getItem(key) as T | null
    return v && allowed.includes(v) ? v : fallback
  } catch {
    return fallback
  }
}

export function remember(key: string, value: string): void {
  try {
    localStorage.setItem(key, value)
  } catch {
    /* storage unavailable: the choice lasts until the window closes */
  }
}

// ---------------------------------------------------------------- keys

const MAC_KEYS: Record<string, string> = {
  Mod: '⌘',
  Command: '⌘',
  Cmd: '⌘',
  CommandOrControl: '⌘',
  CmdOrCtrl: '⌘',
  Control: '⌃',
  Ctrl: '⌃',
  Alt: '⌥',
  Option: '⌥',
  Shift: '⇧',
  Super: '⌘',
  Meta: '⌘',
  Enter: '↵',
  Return: '↵',
  Backspace: '⌫',
  Delete: '⌦',
  Up: '↑',
  Down: '↓',
  Left: '←',
  Right: '→',
  Esc: 'esc'
}
const WIN_KEYS: Record<string, string> = {
  Mod: 'Ctrl',
  Command: 'Win',
  Cmd: 'Win',
  CommandOrControl: 'Ctrl',
  CmdOrCtrl: 'Ctrl',
  Control: 'Ctrl',
  Super: 'Win',
  Meta: 'Win',
  Return: 'Enter',
  Delete: 'Del',
  Up: '↑',
  Down: '↓',
  Left: '←',
  Right: '→'
}

/** "Mod+Shift+C" or an Electron accelerator, as platform key labels. */
export const keyLabels = (combo: string): string[] =>
  combo.split('+').map((k) => (isMac() ? MAC_KEYS : WIN_KEYS)[k] ?? k)

const CODE_KEYS: Record<string, string> = {
  Space: 'Space',
  Tab: 'Tab',
  Enter: 'Enter',
  Backspace: 'Backspace',
  Delete: 'Delete',
  Insert: 'Insert',
  Home: 'Home',
  End: 'End',
  PageUp: 'PageUp',
  PageDown: 'PageDown',
  ArrowUp: 'Up',
  ArrowDown: 'Down',
  ArrowLeft: 'Left',
  ArrowRight: 'Right',
  Backquote: '`',
  Minus: '-',
  Equal: '=',
  BracketLeft: '[',
  BracketRight: ']',
  Backslash: '\\',
  Semicolon: ';',
  Quote: "'",
  Comma: ',',
  Period: '.',
  Slash: '/'
}

/**
 * Electron accelerator for a key press, or an error message.
 * Returns null while only modifiers are held.
 */
export function accelerator(e: KeyboardEvent): { value: string } | { error: string } | null {
  const c = e.code
  const key = /^Key[A-Z]$/.test(c)
    ? c.slice(3)
    : /^Digit\d$/.test(c)
      ? c.slice(5)
      : /^F\d{1,2}$/.test(c)
        ? c
        : CODE_KEYS[c]
  if (!key) return null
  const mods: string[] = []
  if (e.metaKey) mods.push(isMac() ? 'Command' : 'Super')
  if (e.ctrlKey) mods.push('Control')
  if (e.altKey) mods.push('Alt')
  if (e.shiftKey) mods.push('Shift')
  const strong = mods.some((m) => m !== 'Shift')
  if (!strong && !/^F\d/.test(key))
    return {
      error: `Include ${isMac() ? '⌘, ⌃ or ⌥' : 'Ctrl, Alt or Win'} so the shortcut doesn’t clash with typing.`
    }
  return { value: [...mods, key].join('+') }
}

/** Shortcut shown for "Move to trash"; both Delete and Backspace work everywhere. */
export const trashKey = (): string => (isMac() ? 'Mod+Backspace' : 'Mod+Delete')
