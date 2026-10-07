import type { CSSProperties, ReactNode } from 'react'
import type { OcrLine, Settings, Shot } from '../../../shared/types'
import { isMac, keyLabels } from '../lib/util'

/** Keycaps for "Mod+Shift+C" or an Electron accelerator. */
export function Kbd({ combo }: { combo: string }): React.JSX.Element {
  return (
    <span className="kbd" aria-label={combo.replace(/\+/g, ' ')}>
      {keyLabels(combo).map((k, i) => (
        <kbd key={i}>{k}</kbd>
      ))}
    </span>
  )
}

export function Switch(props: {
  checked: boolean
  onChange(v: boolean): void
  label: ReactNode
  hint?: ReactNode
}): React.JSX.Element {
  return (
    <label className="row switch-row">
      <span className="row-text">
        <span className="row-label">{props.label}</span>
        {props.hint && <span className="row-hint">{props.hint}</span>}
      </span>
      <input
        type="checkbox"
        role="switch"
        className="switch"
        checked={props.checked}
        onChange={(e) => props.onChange(e.target.checked)}
      />
    </label>
  )
}

/** "Where to look": chosen folders or every image on the computer. Used by onboarding and settings. */
export function ScopeChoice(props: {
  value: Settings['scope']
  onChange(v: Settings['scope']): void
  /** Labels for folders and everywhere. */
  labels: [string, string]
  foldersHint: string
  labelledBy: string
}): React.JSX.Element {
  const everywhereHint = `${
    isMac()
      ? 'Every image in your home folder, except app data.'
      : 'Every image on your drives, except system, program and app-data folders.'
  } New images are found by name at once; text and visual search fill in over time.`
  return (
    <div className="choices" role="radiogroup" aria-labelledby={props.labelledBy}>
      {(['folders', 'everywhere'] as const).map((v, i) => (
        <label className="row choice card" key={v}>
          <input
            type="radio"
            className="radio"
            name="scope"
            checked={props.value === v}
            onChange={() => props.onChange(v)}
          />
          <span className="row-text">
            <span className="row-label">{props.labels[i]}</span>
            <span className="row-hint">{v === 'folders' ? props.foldersHint : everywhereHint}</span>
          </span>
        </label>
      ))}
    </div>
  )
}

const pct = (n: number): string => `${(n * 100).toFixed(3)}%`
const clamp = (v: number, lo: number, hi: number): number => Math.min(hi, Math.max(lo, v))

/** Tile aspect ratio; keep in sync with `.thumb` in styles.css. */
const TILE = 16 / 10

/**
 * Thumbnail cropped to cover a 16:10 tile. For text hits it is "inked" (dimmed) and centred
 * on the first match, zooming in when the match is tiny; every matched box is re-lit by a copy
 * of the image clipped to that box.
 */
export function Thumb(props: {
  shot: Shot
  highlights: OcrLine[]
  inked: boolean
}): React.JSX.Element {
  const { shot, highlights, inked } = props
  // Unread images have no size yet: fill the tile and let object-fit crop.
  const a = shot.width && shot.height ? shot.width / shot.height : TILE
  const bw = a >= TILE ? a / TILE : 1
  const bh = a >= TILE ? 1 : TILE / a
  const first = inked ? highlights[0] : undefined
  const cx = first ? first.x + first.w / 2 : 0.5
  const cy = first ? first.y + first.h / 2 : a < 1 ? 0.3 : 0.5
  const z = first ? clamp(0.3 / Math.max(0.001, first.w * bw), 1, 2.4) : 1
  const W = bw * z
  const H = bh * z
  const style: CSSProperties = {
    width: pct(W),
    height: pct(H),
    left: pct(clamp(0.5 - cx * W, 1 - W, 0)),
    top: pct(clamp(0.5 - cy * H, 1 - H, 0))
  }
  return (
    <div className="thumb">
      <div className="thumb-canvas" style={style}>
        <img
          src={shot.thumb}
          alt=""
          draggable={false}
          loading="lazy"
          decoding="async"
          data-inked={inked || undefined}
        />
        {inked && <span className="ink" />}
        {inked &&
          highlights.map((h, i) => <span key={i} className="magpie" style={magpie(h, shot.thumb)} />)}
      </div>
    </div>
  )
}

/** A box that shows the un-inked image under one highlight. */
function magpie(h: OcrLine, src: string): CSSProperties {
  const px = 0.004
  const py = h.h * 0.22
  const x = Math.max(0, h.x - px)
  const y = Math.max(0, h.y - py)
  const w = Math.min(1 - x, h.w + 2 * px)
  const hh = Math.min(1 - y, h.h + 2 * py)
  return {
    left: pct(x),
    top: pct(y),
    width: pct(w),
    height: pct(hh),
    backgroundImage: `url("${src}")`,
    backgroundSize: `${100 / w}% ${100 / hh}%`,
    backgroundPosition: `${w < 1 ? (x / (1 - w)) * 100 : 0}% ${hh < 1 ? (y / (1 - hh)) * 100 : 0}%`
  }
}
