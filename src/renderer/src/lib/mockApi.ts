// Browser-only stand-in for the preload API, so the renderer can be built and reviewed with
// `npm run dev:web`. Fake screenshots are SVGs whose OCR boxes are computed from the same
// numbers used to draw the text, so highlights land on the drawn words.
import type {
  ActiveFilter,
  FolderSuggestion,
  MagpieApi,
  IndexStatus,
  MatchKind,
  OcrLine,
  SearchHit,
  SearchRequest,
  SearchResponse,
  Settings,
  Shot,
  ShotDetail,
  ShotMetadata,
  SmartAction,
  SortOrder,
  Stats
} from '../../../shared/types'

// ---------------------------------------------------------------- drawing

const SANS = 'Segoe UI, Helvetica Neue, Arial, sans-serif'
const MONO = 'Cascadia Mono, Consolas, Menlo, monospace'

/** Rough advance width of one character, in em, for the sans stack. */
function charW(ch: string): number {
  if (ch === ' ') return 0.28
  if ("il.,:;|!'`".includes(ch)) return 0.26
  if ('fjrt()[]-/'.includes(ch)) return 0.35
  if ('mwMW@₹'.includes(ch)) return 0.8
  if (ch >= 'A' && ch <= 'Z') return 0.64
  if (ch >= '0' && ch <= '9') return 0.56
  return 0.52
}

function textW(t: string, size: number, mono = false, bold = false): number {
  if (mono) return t.length * 0.6 * size
  let w = 0
  for (const ch of t) w += charW(ch)
  return w * size * (bold ? 1.06 : 1)
}

const esc = (s: string): string =>
  s.replace(/&/g, '&amp;').replace(/</g, '&lt;').replace(/>/g, '&gt;').replace(/"/g, '&quot;')

interface TextOpts {
  weight?: number
  mono?: boolean
  /** OCR text for this run; false to leave it out of the OCR result. */
  ocr?: string | false
  align?: 'end' | 'middle'
}

/** Per-character advance, kept so word highlights can be placed inside a line box. */
interface LineMeta {
  line: OcrLine
  size: number
  mono: boolean
  bold: boolean
  px: number
}

class Canvas {
  parts: string[] = []
  lines: LineMeta[] = []
  constructor(
    readonly w: number,
    readonly h: number,
    bg: string
  ) {
    this.rect(0, 0, w, h, bg)
  }
  rect(x: number, y: number, w: number, h: number, fill: string, r = 0, extra = ''): this {
    this.parts.push(
      `<rect x="${x}" y="${y}" width="${w}" height="${h}" rx="${r}" fill="${fill}" ${extra}/>`
    )
    return this
  }
  circle(cx: number, cy: number, r: number, fill: string): this {
    this.parts.push(`<circle cx="${cx}" cy="${cy}" r="${r}" fill="${fill}"/>`)
    return this
  }
  poly(points: string, fill: string): this {
    this.parts.push(`<polygon points="${points}" fill="${fill}"/>`)
    return this
  }
  /** Draws one run of text with a forced width and records its OCR box. Returns the width. */
  text(t: string, x: number, y: number, size: number, fill: string, o: TextOpts = {}): number {
    const bold = (o.weight ?? 400) >= 600
    const w = textW(t, size, o.mono, bold)
    const left = o.align === 'end' ? x - w : o.align === 'middle' ? x - w / 2 : x
    this.parts.push(
      `<text x="${left}" y="${y}" font-size="${size}" fill="${fill}" font-family="${o.mono ? MONO : SANS}" font-weight="${o.weight ?? 400}" textLength="${w}" lengthAdjust="spacing">${esc(t)}</text>`
    )
    if (o.ocr !== false && t.trim()) {
      const ocr = o.ocr ?? t
      this.lines.push({
        line: {
          t: ocr,
          x: left / this.w,
          y: (y - size * 0.86) / this.h,
          w: w / this.w,
          h: (size * 1.14) / this.h
        },
        size,
        mono: !!o.mono,
        bold,
        px: left
      })
    }
    return w
  }
  url(): string {
    const svg = `<svg xmlns="http://www.w3.org/2000/svg" width="${this.w}" height="${this.h}" viewBox="0 0 ${this.w} ${this.h}">${this.parts.join('')}</svg>`
    return `data:image/svg+xml;charset=utf-8,${encodeURIComponent(svg)}`
  }
}

function wrap(t: string, n: number): string[] {
  const out: string[] = []
  let line = ''
  for (const w of t.split(' ')) {
    if (line && (line + ' ' + w).length > n) {
      out.push(line)
      line = w
    } else line = line ? line + ' ' + w : w
  }
  if (line) out.push(line)
  return out
}

function hash(s: string): number {
  let h = 7
  for (const ch of s) h = (h * 31 + ch.charCodeAt(0)) | 0
  return Math.abs(h)
}

// ---------------------------------------------------------------- scenes

type Msg = [name: string, time: string, text: string]
const AVATARS = ['#5865f2', '#eb459e', '#3ba55c', '#faa61a', '#ed4245', '#00a8fc']

function discord(channel: string, msgs: Msg[]): Canvas {
  const c = new Canvas(1440, 900, '#313338')
  c.rect(0, 0, 72, 900, '#1e1f22')
  for (let i = 0; i < 6; i++) c.circle(36, 38 + i * 60, 24, i === 1 ? '#5865f2' : '#2b2d31')
  c.rect(72, 0, 240, 900, '#2b2d31')
  c.text('Magpie Crew', 88, 31, 15, '#f2f3f5', { weight: 600 })
  c.rect(72, 48, 240, 1, '#1f2023')
  ;['general', 'design', 'dev', 'random', 'screenshots'].forEach((ch, i) => {
    const y = 92 + i * 34
    if (ch === channel) c.rect(80, y - 21, 224, 30, '#404249', 4)
    c.text(`# ${ch}`, 92, y, 15, ch === channel ? '#f2f3f5' : '#949ba4')
  })
  c.rect(312, 48, 1128, 1, '#26272b')
  c.text(`# ${channel}`, 332, 31, 16, '#f2f3f5', { weight: 600 })
  let y = 100
  for (const [name, time, text] of msgs) {
    c.circle(352, y + 6, 20, AVATARS[hash(name) % AVATARS.length])
    const nw = c.text(name, 388, y, 16, '#f2f3f5', { weight: 600 })
    c.text(time, 388 + nw + 10, y, 12, '#949ba4')
    const ls = wrap(text, 96)
    ls.forEach((l, i) => c.text(l, 388, y + 27 + i * 24, 15.5, '#dbdee1'))
    y += 36 + ls.length * 24 + 16
  }
  c.rect(332, 830, 1088, 48, '#383a40', 8)
  c.text(`Message #${channel}`, 352, 860, 15, '#6d6f78')
  return c
}

function slack(channel: string, msgs: Msg[]): Canvas {
  const c = new Canvas(1440, 900, '#ffffff')
  c.rect(0, 0, 260, 900, '#3f0e40')
  c.text('Northwind', 20, 34, 17, '#ffffff', { weight: 700 })
  c.rect(0, 54, 260, 1, '#5d2c5d')
  ;['announcements', 'design', 'eng', 'general', 'planning'].forEach((ch, i) => {
    const y = 96 + i * 30
    if (ch === channel) c.rect(8, y - 20, 244, 28, '#1164a3', 6)
    c.text(`# ${ch}`, 20, y, 15, ch === channel ? '#ffffff' : '#cfc3cf')
  })
  c.text(`# ${channel}`, 284, 34, 18, '#1d1c1d', { weight: 700 })
  c.rect(260, 54, 1180, 1, '#e2e2e2')
  let y = 104
  for (const [name, time, text] of msgs) {
    c.rect(284, y - 18, 38, 38, AVATARS[hash(name) % AVATARS.length], 6)
    const nw = c.text(name, 336, y, 15.5, '#1d1c1d', { weight: 700 })
    c.text(time, 336 + nw + 8, y, 12, '#616061')
    const ls = wrap(text, 100)
    ls.forEach((l, i) => c.text(l, 336, y + 25 + i * 23, 15, '#1d1c1d'))
    y += 34 + ls.length * 23 + 18
  }
  c.rect(284, 812, 1132, 66, '#ffffff', 8, 'stroke="#bbbabb"')
  c.text(`Message #${channel}`, 300, 850, 15, '#868686')
  return c
}

const KW =
  /^(const|let|var|function|return|import|from|export|async|await|if|else|for|in|of|def|class|with|as|interface|type|new|SELECT|FROM|WHERE|AND|GROUP|BY|ORDER|LIMIT|AS|DESC|SUM|NOW|INTERVAL)$/

function code(file: string, src: string[], tree: string[]): Canvas {
  const c = new Canvas(1440, 900, '#1f1f1f')
  c.rect(0, 0, 48, 900, '#181818')
  c.rect(48, 0, 240, 900, '#181818')
  c.text('EXPLORER', 64, 28, 11, '#9d9d9d', { weight: 600 })
  tree.forEach((f, i) => {
    if (f === file) c.rect(48, 44 + i * 24, 240, 24, '#37373d')
    c.text(f, 76, 61 + i * 24, 13, f === file ? '#ffffff' : '#cccccc')
  })
  c.rect(288, 0, 1152, 36, '#181818')
  c.rect(288, 0, 190, 36, '#1f1f1f')
  c.rect(288, 0, 190, 1, '#0078d4')
  c.text(file, 306, 23, 13, '#ffffff')
  const size = 14.5
  const cw = size * 0.6
  src.forEach((line, i) => {
    const y = 72 + i * 23
    c.text(String(i + 1), 326, y, 13, '#6e7681', { mono: true, align: 'end', ocr: false })
    if (!line.trim()) return
    const trimmed = line.trim()
    const comment = trimmed.startsWith('//') || trimmed.startsWith('#') || trimmed.startsWith('--')
    let col = 0
    const segs = comment ? [line] : line.split(/("[^"]*"|'[^']*'|\b\w+\b)/)
    for (const s of segs) {
      if (!s) continue
      const color = comment
        ? '#6a9955'
        : /^["']/.test(s)
          ? '#ce9178'
          : KW.test(s)
            ? '#569cd6'
            : /^\d/.test(s)
              ? '#b5cea8'
              : /^[A-Z]/.test(s)
                ? '#4ec9b0'
                : '#cccccc'
      if (s.trim()) c.text(s, 350 + col * cw, y, size, color, { mono: true, ocr: false })
      col += s.length
    }
    // One OCR line per source line, like a real OCR engine would return.
    const lead = line.length - line.trimStart().length
    c.lines.push({
      line: {
        t: trimmed,
        x: (350 + lead * cw) / c.w,
        y: (y - size * 0.86) / c.h,
        w: (trimmed.length * cw) / c.w,
        h: (size * 1.14) / c.h
      },
      size,
      mono: true,
      bold: false,
      px: 350 + lead * cw
    })
  })
  c.rect(0, 878, 1440, 22, '#181818')
  c.rect(0, 878, 40, 22, '#0078d4')
  c.text('main', 56, 893, 12, '#cccccc')
  c.text(`Ln ${src.length}, Col 1   UTF-8   LF`, 1420, 893, 12, '#cccccc', { align: 'end' })
  return c
}

function terminal(title: string, lines: [string, string?][]): Canvas {
  const c = new Canvas(1280, 720, '#0c0c0c')
  c.rect(0, 0, 1280, 38, '#1f1f1f')
  c.rect(8, 6, 230, 32, '#0c0c0c', 6)
  c.text(title, 24, 27, 12.5, '#cccccc')
  lines.forEach(([t, color], i) =>
    c.text(t, 18, 72 + i * 22, 15, color ?? '#cccccc', { mono: true })
  )
  return c
}

interface Inv {
  vendor: string
  addr: string[]
  heading: string
  ocrHeading?: string
  meta: string[]
  bill: string[]
  items: [string, string, string][]
  totals: [string, string][]
  foot: string[]
}

function invoice(o: Inv): Canvas {
  const c = new Canvas(900, 1200, '#ffffff')
  c.rect(0, 0, 900, 8, '#1f2937')
  c.text(o.vendor, 70, 110, 28, '#111827', { weight: 700 })
  o.addr.forEach((a, i) => c.text(a, 70, 142 + i * 22, 14, '#6b7280'))
  c.text(o.heading, 830, 110, 34, '#111827', { weight: 700, align: 'end', ocr: o.ocrHeading })
  o.meta.forEach((m, i) => c.text(m, 830, 146 + i * 22, 14, '#374151', { align: 'end' }))
  c.text('Bill to', 70, 290, 13, '#6b7280', { weight: 600 })
  o.bill.forEach((b, i) => c.text(b, 70, 316 + i * 22, 15, '#111827'))
  c.rect(70, 420, 760, 40, '#f3f4f6', 4)
  c.text('Description', 86, 446, 13, '#374151', { weight: 600 })
  c.text('Qty', 620, 446, 13, '#374151', { weight: 600, align: 'end' })
  c.text('Amount', 814, 446, 13, '#374151', { weight: 600, align: 'end' })
  let y = 500
  for (const [d, q, a] of o.items) {
    c.text(d, 86, y, 15, '#111827')
    c.text(q, 620, y, 15, '#111827', { align: 'end' })
    c.text(a, 814, y, 15, '#111827', { align: 'end' })
    c.rect(70, y + 18, 760, 1, '#e5e7eb')
    y += 46
  }
  y += 20
  o.totals.forEach(([k, v], i) => {
    const last = i === o.totals.length - 1
    c.text(k, 600, y, last ? 18 : 15, '#111827', { weight: last ? 700 : 400 })
    c.text(v, 814, y, last ? 18 : 15, '#111827', { weight: last ? 700 : 400, align: 'end' })
    y += last ? 0 : 32
  })
  o.foot.forEach((f, i) => c.text(f, 450, 1080 + i * 26, 14, '#6b7280', { align: 'middle' }))
  return c
}

function sms(contact: string, msgs: [string, string][]): Canvas {
  const c = new Canvas(390, 844, '#ffffff')
  c.text('9:41', 34, 34, 16, '#000000', { weight: 600, ocr: false })
  c.rect(318, 22, 42, 15, '#000000', 4)
  c.rect(0, 52, 390, 76, '#f7f7f7')
  c.circle(195, 78, 18, '#a0a4ab')
  c.text(contact, 195, 116, 12.5, '#000000', { align: 'middle' })
  c.rect(0, 128, 390, 1, '#d8d8d8')
  let y = 170
  for (const [time, text] of msgs) {
    c.text(time, 195, y, 11, '#8a8a8e', { align: 'middle' })
    y += 22
    const ls = wrap(text, 30)
    const bw = Math.max(...ls.map((l) => textW(l, 16))) + 28
    c.rect(14, y - 4, bw, ls.length * 21 + 18, '#e9e9eb', 18)
    ls.forEach((l, i) => c.text(l, 28, y + 18 + i * 21, 16, '#000000'))
    y += ls.length * 21 + 40
  }
  c.rect(14, 784, 362, 36, '#ffffff', 18, 'stroke="#d1d1d6"')
  c.text('Text Message', 30, 808, 15, '#c7c7cc', { ocr: false })
  return c
}

function dialog(
  app: string,
  title: string,
  body: string[],
  buttons: string[],
  warn = false
): Canvas {
  const c = new Canvas(1280, 800, '#24435f')
  c.rect(0, 760, 1280, 40, '#1b1b1b')
  c.rect(330, 230, 620, 300, '#f3f3f3', 8, 'stroke="#c8c8c8"')
  c.text(app, 350, 258, 12, '#5c5c5c')
  c.circle(386, 316, 22, warn ? '#f7b500' : '#c42b1c')
  c.text(warn ? '!' : '×', 386, 326, 28, '#ffffff', { weight: 700, align: 'middle', ocr: false })
  c.text(title, 426, 312, 18, '#1b1b1b', { weight: 600 })
  body.forEach((b, i) => c.text(b, 426, 346 + i * 24, 14.5, '#1b1b1b'))
  c.rect(330, 452, 620, 78, '#e8e8e8', 0)
  let x = 926
  buttons.forEach((b, i) => {
    const w = Math.max(96, textW(b, 14) + 36)
    c.rect(x - w, 472, w, 34, i === 0 ? '#0067c0' : '#fbfbfb', 4, 'stroke="#c8c8c8"')
    c.text(b, x - w / 2, 494, 14, i === 0 ? '#ffffff' : '#1b1b1b', { align: 'middle' })
    x -= w + 10
  })
  return c
}

function web(url: string, site: string, title: string, sub: string, paras: string[]): Canvas {
  const c = new Canvas(1440, 900, '#ffffff')
  c.rect(0, 0, 1440, 86, '#dfe3e8')
  c.rect(10, 8, 250, 32, '#ffffff', 8)
  c.text(title.slice(0, 26), 28, 29, 12.5, '#1f1f1f', { ocr: false })
  c.rect(100, 48, 1240, 30, '#ffffff', 15)
  c.text(url, 128, 68, 14, '#1f1f1f')
  c.rect(0, 86, 1440, 60, '#ffffff')
  c.rect(0, 146, 1440, 1, '#e6e6e6')
  c.text(site, 160, 124, 19, '#111111', { weight: 700 })
  ;['Docs', 'Blog', 'About'].forEach((n, i) => c.text(n, 1100 + i * 80, 122, 14, '#555555'))
  let y = 232
  for (const l of wrap(title, 46)) {
    c.text(l, 160, y, 38, '#111111', { weight: 700 })
    y += 48
  }
  if (sub) {
    c.text(sub, 160, y + 4, 18, '#555555')
    y += 40
  }
  y += 24
  for (const p of paras) {
    for (const l of wrap(p, 108)) {
      c.text(l, 160, y, 17, '#2b2b2b')
      y += 30
    }
    y += 16
  }
  return c
}

function photo(kind: string): Canvas {
  const W = 1600
  const H = 1000
  switch (kind) {
    case 'sunset': {
      const c = new Canvas(W, H, '#f29b4b')
      c.rect(0, 0, W, 220, '#f5b25c').rect(0, 420, W, 200, '#e9714a')
      c.circle(800, 610, 130, '#ffe08a')
      c.rect(0, 610, W, 390, '#3b2e5a')
      for (let i = 0; i < 7; i++) c.rect(700 - i * 10, 640 + i * 40, 200 + i * 20, 6, '#ffcf70', 3)
      return c
    }
    case 'mountains': {
      const c = new Canvas(W, H, '#bcd7f0')
      c.poly('0,720 380,260 760,720', '#5a6f8c').poly('520,720 1000,180 1480,720', '#7d93ad')
      c.poly('1000,180 1090,290 910,290', '#ffffff').poly('380,260 450,350 310,350', '#ffffff')
      c.rect(0, 720, W, 280, '#4f7aa3')
      return c
    }
    case 'city': {
      const c = new Canvas(W, H, '#0f1424')
      for (let i = 0; i < 12; i++) {
        const x = i * 135
        const h = 300 + ((i * 97) % 420)
        c.rect(x, H - h, 120, h, '#1c2438')
        for (let r = 0; r < h / 50 - 1; r++)
          for (let k = 0; k < 3; k++)
            if ((i + r + k) % 3) c.rect(x + 16 + k * 34, H - h + 24 + r * 50, 18, 24, '#f6c66b')
      }
      c.circle(1380, 150, 50, '#f4f1e6')
      return c
    }
    case 'beach': {
      const c = new Canvas(W, H, '#8fd3f4')
      c.rect(0, 430, W, 230, '#2a9fd6').rect(0, 660, W, 340, '#f1d9a7')
      c.poly('1100,560 1300,560 1200,480', '#e94b3c').rect(1196, 560, 8, 200, '#6b4f3a')
      c.circle(300, 160, 70, '#fff3b0')
      return c
    }
    case 'cat': {
      const c = new Canvas(W, H, '#3c6e91')
      c.rect(0, 760, W, 240, '#2b506a')
      c.circle(800, 640, 230, '#e8954a').circle(800, 380, 150, '#e8954a')
      c.poly('670,300 700,160 770,260', '#e8954a').poly('930,300 900,160 830,260', '#e8954a')
      c.circle(745, 370, 16, '#1e2a1e').circle(855, 370, 16, '#1e2a1e')
      c.poly('785,420 815,420 800,440', '#c46a6a')
      return c
    }
    case 'dog': {
      const c = new Canvas(W, H, '#9cc56b')
      c.rect(0, 700, W, 300, '#6f9a45')
      c.circle(800, 620, 220, '#a8743f').circle(800, 360, 160, '#a8743f')
      c.circle(650, 330, 70, '#6e4a26').circle(950, 330, 70, '#6e4a26')
      c.circle(745, 350, 16, '#1b1b1b')
        .circle(855, 350, 16, '#1b1b1b')
        .circle(800, 430, 26, '#1b1b1b')
      return c
    }
    case 'pizza': {
      const c = new Canvas(W, H, '#7b5236')
      c.circle(800, 500, 380, '#d99a4e').circle(800, 500, 330, '#f0c260')
      for (let i = 0; i < 9; i++)
        c.circle(800 + Math.cos(i * 0.7) * 210, 500 + Math.sin(i * 0.7) * 210, 42, '#b8322a')
      c.circle(800, 500, 50, '#b8322a')
      return c
    }
    case 'forest': {
      const c = new Canvas(W, H, '#cfe3d4')
      for (let i = 0; i < 14; i++) {
        const x = i * 120 + 20
        const s = 1 + (i % 3) * 0.25
        c.poly(
          `${x},${900} ${x + 60},${900 - 420 * s} ${x + 120},${900}`,
          i % 2 ? '#2f5d3a' : '#3f7a4a'
        )
      }
      c.rect(0, 900, W, 100, '#24432b')
      return c
    }
    default: {
      const c = new Canvas(W, H, '#2f4a2c')
      for (let i = 0; i < 18; i++) {
        const x = 120 + ((i * 263) % 1380)
        const y = 140 + ((i * 181) % 720)
        c.circle(x, y, 60, i % 3 ? '#d6336c' : '#f06595').circle(x, y, 18, '#ffd43b')
      }
      return c
    }
  }
}

function mapCard(title: string, rows: string[]): Canvas {
  const c = new Canvas(390, 844, '#e8eaed')
  c.rect(0, 0, 390, 844, '#e8eaed')
  c.rect(-20, 180, 440, 26, '#ffffff')
    .rect(140, 0, 22, 600, '#ffffff')
    .rect(0, 380, 390, 18, '#ffffff')
  c.rect(220, 220, 150, 140, '#c8e6c9', 8).rect(20, 420, 110, 160, '#aadaff', 8)
  c.circle(196, 300, 16, '#d93025').circle(196, 300, 6, '#ffffff')
  c.rect(0, 560, 390, 284, '#ffffff', 16)
  c.rect(175, 572, 40, 5, '#d0d0d0', 3)
  c.text(title, 20, 620, 20, '#202124', { weight: 600 })
  rows.forEach((r, i) => c.text(r, 20, 654 + i * 28, 15, i === 2 ? '#188038' : '#5f6368'))
  return c
}

function sheet(title: string, head: string[], rows: string[][]): Canvas {
  const c = new Canvas(1440, 900, '#ffffff')
  c.rect(0, 0, 1440, 48, '#217346')
  c.text(title, 20, 31, 15, '#ffffff', { weight: 600 })
  c.rect(0, 48, 1440, 40, '#f3f3f3')
  c.text('fx', 20, 74, 13, '#666666', { ocr: false })
  const colW = [60, 260, 160, 160, 160, 180]
  const xs = colW.reduce<number[]>((a, w) => [...a, a[a.length - 1] + w], [0])
  const all = [head, ...rows]
  all.forEach((r, ri) => {
    const y = 120 + ri * 34
    c.rect(0, y - 22, 1440, 1, '#e1e1e1')
    c.text(String(ri + 1), 30, y, 12, '#666666', { align: 'middle', ocr: false })
    r.forEach((cell, ci) => {
      const right = ci > 0
      c.text(cell, right ? xs[ci + 2] - 12 : xs[1] + 10, y, 14, '#1f1f1f', {
        align: right ? 'end' : undefined,
        weight: ri === 0 || ri === all.length - 1 ? 600 : 400
      })
    })
  })
  xs.forEach((x) => c.rect(x, 98, 1, all.length * 34, '#e1e1e1'))
  return c
}

function palette(title: string, sw: [string, string][]): Canvas {
  const c = new Canvas(1440, 900, '#1e1e1e')
  c.rect(0, 0, 1440, 48, '#2c2c2c')
  c.text(title, 20, 30, 13, '#ffffff', { weight: 600 })
  c.rect(160, 120, 1120, 660, '#ffffff', 4)
  c.text(title, 200, 180, 28, '#111111', { weight: 700 })
  sw.forEach(([name, hex], i) => {
    const x = 200 + (i % 3) * 350
    const y = 230 + Math.floor(i / 3) * 260
    c.rect(x, y, 310, 160, hex, 12)
    c.text(name, x, y + 194, 16, '#111111', { weight: 600 })
    c.text(hex.toUpperCase(), x, y + 220, 14, '#555555', { mono: true })
  })
  return c
}

function slides(bullets: string[]): Canvas {
  const c = new Canvas(1440, 900, '#202124')
  c.rect(40, 60, 1040, 585, '#ffffff', 6)
  c.text('Q3 Roadmap Review', 100, 160, 44, '#1a1a1a', { weight: 700 })
  bullets.forEach((b, i) => {
    c.circle(112, 233 + i * 64, 6, '#e0703a')
    c.text(b, 136, 240 + i * 64, 26, '#333333')
  })
  ;['Lena', 'Omar', 'Ana', 'You'].forEach((n, i) => {
    c.rect(1110, 60 + i * 150, 290, 138, '#3c4043', 8)
    c.circle(1255, 116 + i * 150, 28, AVATARS[i])
    c.text(n, 1124, 186 + i * 150, 14, '#ffffff')
  })
  c.rect(560, 820, 320, 56, '#303134', 28)
  return c
}

// ---------------------------------------------------------------- library

interface Def {
  c: Canvas
  age: number // hours ago
  folder: number
  kind: string
  tags: string[]
  colors: string[]
  name?: string
  pinned?: boolean
  group?: string
}

const FOLDERS = [
  'C:\\Users\\Alex\\Pictures\\Screenshots',
  'C:\\Users\\Alex\\Desktop',
  'C:\\Users\\Alex\\Pictures\\Discord'
]

const SEARCH_TS = [
  "import { useEffect, useRef, useState } from 'react'",
  '',
  '// Debounce the query so we search at most every 60 ms',
  'export function useDebounced<T>(value: T, ms = 60): T {',
  '  const [v, setV] = useState(value)',
  '  useEffect(() => {',
  '    const t = setTimeout(() => setV(value), ms)',
  '    return () => clearTimeout(t)',
  '  }, [value, ms])',
  '  return v',
  '}',
  '',
  'export async function search(q: string) {',
  '  const seq = ++counter',
  '  const res = await window.api.search({ q })',
  '  if (seq !== counter) return null // stale response',
  '  return res',
  '}'
]
const TREE = ['App.tsx', 'search.ts', 'Thumb.tsx', 'tokens.css', 'mockApi.ts', 'package.json']

function defs(): Def[] {
  const chatTags = ['chat', 'conversation', 'messages']
  const codeTags = ['code', 'editor', 'programming']
  const termTags = ['terminal', 'console', 'code', 'error']
  const recTags = ['receipt', 'invoice', 'document', 'bill']
  const smsTags = ['phone', 'message', 'sms', 'otp']
  const dlgTags = ['error', 'dialog', 'warning', 'popup']
  const webTags = ['website', 'web page', 'article', 'browser']
  const DC = ['#313338', '#2b2d31', '#5865f2']
  const SL = ['#ffffff', '#3f0e40', '#1164a3']
  const VS = ['#1f1f1f', '#181818', '#569cd6']
  const TM = ['#0c0c0c', '#1f1f1f', '#cccccc']
  const INV = ['#ffffff', '#f3f4f6', '#1f2937']
  const SMS = ['#ffffff', '#e9e9eb', '#f7f7f7']
  const DLG = ['#24435f', '#f3f3f3', '#c42b1c']
  const WEB = ['#ffffff', '#dfe3e8', '#111111']

  const d: Def[] = []
  // A burst: four shots of the same editor a few seconds apart.
  ;[11, 14, 16, 18].forEach((n, i) =>
    d.push({
      c: code('search.ts', SEARCH_TS.slice(0, n), TREE),
      age: 0.32 - i * 0.004,
      folder: 0,
      kind: 'code',
      tags: codeTags,
      colors: VS,
      group: 'code'
    })
  )
  d.push(
    {
      c: discord('design', [
        [
          'maya',
          'Today at 11:02 AM',
          'can someone send me the invoice for the September retainer? need it for the accounts close'
        ],
        [
          'devon',
          'Today at 11:04 AM',
          'sure, invoice INV-2041 is in the drive. total is $1,284.00 including tax'
        ],
        [
          'maya',
          'Today at 11:05 AM',
          'perfect, thanks. the new design file is at figma.com/file/magpie-search-ui'
        ],
        ['rio', 'Today at 11:09 AM', 'the amber accent looks great on the dark theme, ship it']
      ]),
      age: 1.2,
      folder: 2,
      kind: 'chat',
      tags: [...chatTags, 'discord'],
      colors: DC
    },
    {
      c: sms('57575', [
        [
          'Today 9:38 AM',
          'Your verification code is 482913. It expires in 10 minutes. Do not share this code with anyone.'
        ]
      ]),
      age: 2,
      folder: 0,
      kind: 'sms',
      tags: smsTags,
      colors: SMS
    },
    {
      c: dialog(
        'magpie-indexer.exe',
        'Runtime Error',
        [
          'The application was unable to start correctly (0xc000007b).',
          'Click OK to close the application.'
        ],
        ['OK']
      ),
      age: 3,
      folder: 0,
      kind: 'dialog',
      tags: dlgTags,
      colors: DLG
    },
    {
      c: web(
        'https://www.electronjs.org/docs/latest/api/global-shortcut',
        'Electron',
        'globalShortcut',
        'Detect keyboard events when the app does not have focus.',
        [
          'The globalShortcut module registers and unregisters keyboard shortcuts with the operating system, so you can react to them anywhere.',
          'The shortcut is global: it works even when the app is in the background. Register shortcuts only after the app is ready.',
          'globalShortcut.register(accelerator, callback) returns a boolean that tells you whether the shortcut was registered.'
        ]
      ),
      age: 4,
      folder: 0,
      kind: 'web',
      tags: [...webTags, 'docs'],
      colors: WEB
    },
    {
      c: photo('sunset'),
      age: 5,
      folder: 2,
      kind: 'photo',
      tags: ['sunset', 'sky', 'sea', 'ocean', 'evening', 'orange', 'landscape'],
      colors: ['#f29b4b', '#3b2e5a', '#ffe08a'],
      name: 'IMG_2041.jpg'
    },
    {
      c: terminal('PowerShell', [
        ['PS C:\\code\\magpie> npm install'],
        ['npm ERR! code ERESOLVE', '#f14c4c'],
        ['npm ERR! ERESOLVE unable to resolve dependency tree', '#f14c4c'],
        ['npm ERR!', '#f14c4c'],
        ['npm ERR! While resolving: magpie@1.0.0', '#f14c4c'],
        ['npm ERR! Found: react@19.2.1', '#f14c4c'],
        ['npm ERR! Could not resolve dependency:', '#f14c4c'],
        ['npm ERR! peer react@"^18.0.0" from react-virtualized@9.22.5', '#f14c4c'],
        ['npm ERR!', '#f14c4c'],
        ['npm ERR! Fix the upstream dependency conflict, or retry', '#f14c4c'],
        ['npm ERR! this command with --force or --legacy-peer-deps', '#f14c4c'],
        ['PS C:\\code\\magpie>']
      ]),
      age: 6,
      folder: 0,
      kind: 'terminal',
      tags: termTags,
      colors: TM
    },
    {
      c: invoice({
        vendor: 'Northwind Studio',
        addr: ['88 Mission Street, Suite 400', 'San Francisco, CA 94105'],
        heading: 'INVOICE',
        meta: ['Invoice no. INV-2041', 'Date: 28 Sep 2026', 'Due: 12 Oct 2026'],
        bill: ['Magpie Labs', '14 Harbour Road', 'Bengaluru 560001'],
        items: [
          ['Design retainer, September', '1', '$950.00'],
          ['Icon set (24 icons)', '1', '$180.00'],
          ['Usability sessions', '2', '$40.00']
        ],
        totals: [
          ['Subtotal', '$1,170.00'],
          ['Tax', '$114.00'],
          ['Total due', '$1,284.00']
        ],
        foot: [
          'Questions? billing@northwind.io · +1 (415) 555-0132',
          'Pay online: https://pay.northwind.io/inv-2041'
        ]
      }),
      age: 7,
      folder: 1,
      kind: 'receipt',
      tags: recTags,
      colors: INV,
      name: 'invoice-northwind-INV-2041.png',
      pinned: true
    },
    // yesterday
    {
      c: slack('eng', [
        [
          'Lena Park',
          '10:14 AM',
          'deploy to staging failed again. the build step times out after 600s'
        ],
        [
          'Omar Haddad',
          '10:16 AM',
          'looks like the docker cache got evicted. retrying with --no-cache now'
        ],
        ['Lena Park', '10:31 AM', 'green now. logs are at https://ci.northwind.dev/runs/88213']
      ]),
      age: 26,
      folder: 0,
      kind: 'chat',
      tags: [...chatTags, 'slack'],
      colors: SL
    },
    {
      c: sms('BX-FOODIE', [
        [
          'Yesterday 8:12 PM',
          'Your Foodie order OTP is 7741. Share it with the delivery partner only at the door.'
        ]
      ]),
      age: 28,
      folder: 0,
      kind: 'sms',
      tags: smsTags,
      colors: SMS
    },
    {
      c: web(
        'https://www.skyline-air.example/booking/K7XQ2M',
        'Skyline Air',
        'Your booking is confirmed',
        'We have emailed your e-ticket.',
        [
          'Booking reference (PNR): K7XQ2M',
          'SA 2131 · Delhi (DEL) to Bengaluru (BLR) · Sat, 17 Oct 2026',
          'Departs 06:10 from Terminal 1 · Arrives 08:55 at Terminal 2',
          'Passenger: Alex Rivera · Seat 14C · 15 kg check-in baggage',
          'Questions? Write to help@skyline-air.example'
        ]
      ),
      age: 29,
      folder: 0,
      kind: 'web',
      tags: [...webTags, 'flight', 'travel', 'ticket'],
      colors: WEB
    },
    ...[2, 3, 4].map((n, i): Def => ({
      c: slides(
        [
          'Ship semantic search to beta',
          'Cut cold start below 400 ms',
          'Group bursts of screenshots',
          'Windows on ARM build'
        ].slice(0, n)
      ),
      age: 30 - i * 0.01,
      folder: 0,
      kind: 'slides',
      tags: ['presentation', 'slides', 'meeting', 'video call'],
      colors: ['#202124', '#ffffff', '#3c4043'],
      group: 'slides'
    })),
    {
      c: photo('mountains'),
      age: 32,
      folder: 1,
      kind: 'photo',
      tags: ['mountain', 'mountains', 'snow', 'hiking', 'landscape', 'lake', 'blue'],
      colors: ['#bcd7f0', '#5a6f8c', '#4f7aa3'],
      name: 'IMG_1988.jpg'
    },
    {
      c: palette('Magpie tokens', [
        ['Ink 900', '#111014'],
        ['Ink 700', '#2a2730'],
        ['Paper', '#f7f3ea'],
        ['Amber', '#f2b544'],
        ['Ember', '#e0703a'],
        ['Moss', '#6b8f5e']
      ]),
      age: 34,
      folder: 0,
      kind: 'design',
      tags: ['palette', 'colors', 'design', 'swatches'],
      colors: ['#1e1e1e', '#ffffff', '#f2b544']
    },
    // this week
    {
      c: discord('random', [
        ['sam', 'Monday at 6:40 PM', 'office wifi password changed again'],
        ['sam', 'Monday at 6:41 PM', 'network: INKWELL-5G   password: coffee-and-ink-42'],
        ['jules', 'Monday at 6:43 PM', 'saving this screenshot forever'],
        ['devon', 'Monday at 6:50 PM', 'please do not post passwords in #random']
      ]),
      age: 52,
      folder: 2,
      kind: 'chat',
      tags: [...chatTags, 'discord'],
      colors: DC
    },
    {
      c: code(
        'report.sql',
        [
          '-- monthly revenue by plan',
          "SELECT plan, date_trunc('month', paid_at) AS month,",
          '       SUM(amount_cents) / 100.0 AS revenue',
          'FROM invoices',
          "WHERE status = 'paid'",
          "  AND paid_at >= NOW() - INTERVAL '12 months'",
          'GROUP BY plan, month',
          'ORDER BY month DESC, revenue DESC',
          'LIMIT 50;'
        ],
        ['report.sql', 'schema.sql', 'seed.sql']
      ),
      age: 60,
      folder: 0,
      kind: 'code',
      tags: [...codeTags, 'sql', 'database'],
      colors: VS
    },
    {
      c: invoice({
        vendor: 'Blue Tokai Coffee Roasters',
        addr: ['12th Main Rd, Indiranagar', 'Bengaluru 560038'],
        heading: 'Receipt',
        ocrHeading: 'Recelpt',
        meta: ['No. 20931', 'Sun, 4 Oct 2026 10:42', 'Table 6'],
        bill: ['Paid by UPI', 'Server: Anu'],
        items: [
          ['Cappuccino', '2', '₹520'],
          ['Almond croissant', '1', '₹240'],
          ['Cold brew tonic', '1', '₹280']
        ],
        totals: [
          ['Subtotal', '₹1,040'],
          ['GST 5%', '₹52'],
          ['Total', '₹1,092']
        ],
        foot: ['Thank you for visiting', 'bluetokaicoffee.com']
      }),
      age: 64,
      folder: 0,
      kind: 'receipt',
      tags: [...recTags, 'coffee', 'cafe'],
      colors: INV
    },
    {
      c: mapCard('Blue Tokai Coffee Roasters', [
        '4.6 (2,184) · Coffee shop · 1.2 km',
        '12th Main Rd, Indiranagar, Bengaluru',
        'Open · Closes 10 PM',
        'Directions     Call     Save'
      ]),
      age: 66,
      folder: 0,
      kind: 'map',
      tags: ['map', 'location', 'directions', 'place', 'coffee'],
      colors: ['#e8eaed', '#ffffff', '#c8e6c9']
    },
    {
      c: web(
        'https://cooking.example.com/recipes/miso-salmon',
        'Weeknight Kitchen',
        'Miso-glazed salmon with sesame greens',
        'Ready in 25 minutes · Serves 2',
        [
          'Ingredients: 2 salmon fillets, 2 tbsp white miso, 1 tbsp mirin, 1 tsp honey, 1 tsp soy sauce, 1 bunch greens.',
          'Whisk the miso, mirin, honey and soy sauce. Brush over the salmon and rest for 15 minutes.',
          'Broil for 6 to 8 minutes until the glaze caramelizes. Serve with sesame greens and rice.'
        ]
      ),
      age: 70,
      folder: 0,
      kind: 'web',
      tags: [...webTags, 'recipe', 'food', 'cooking'],
      colors: WEB
    },
    {
      c: dialog(
        'Storage',
        'Disk almost full',
        ['Local Disk (C:) has only 1.2 GB free.', 'Free up space to keep indexing screenshots.'],
        ['Free up space', 'Dismiss'],
        true
      ),
      age: 80,
      folder: 0,
      kind: 'dialog',
      tags: dlgTags,
      colors: ['#24435f', '#f3f3f3', '#f7b500']
    },
    {
      c: terminal('bash', [
        ['$ git push origin main'],
        ['To github.com:magpie-app/magpie.git'],
        [' ! [rejected]        main -> main (fetch first)', '#f14c4c'],
        ["error: failed to push some refs to 'github.com:magpie-app/magpie.git'", '#f14c4c'],
        ['hint: Updates were rejected because the remote contains work that you do', '#e5e510'],
        ['hint: not have locally. Integrate the remote changes (e.g.', '#e5e510'],
        ["hint: 'git pull ...') before pushing again.", '#e5e510'],
        ['$']
      ]),
      age: 90,
      folder: 0,
      kind: 'terminal',
      tags: [...termTags, 'git'],
      colors: TM
    },
    {
      c: photo('city'),
      age: 100,
      folder: 1,
      kind: 'photo',
      tags: ['city', 'night', 'skyline', 'lights', 'building', 'buildings'],
      colors: ['#0f1424', '#1c2438', '#f6c66b'],
      name: 'IMG_1932.jpg'
    },
    {
      c: sms('Ridely', [
        ['Tue 7:55 AM', 'Your Ridely PIN is 3920. Share it with your driver to start the trip.']
      ]),
      age: 110,
      folder: 0,
      kind: 'sms',
      tags: smsTags,
      colors: SMS
    },
    {
      c: sheet(
        'Q3 budget.xlsx',
        ['Category', 'Jul', 'Aug', 'Sep', 'Total'],
        [
          ['Hosting', '420', '380', '410', '1,210'],
          ['Design', '950', '950', '950', '2,850'],
          ['Software', '312', '298', '305', '915'],
          ['Travel', '0', '1,240', '180', '1,420'],
          ['Total', '1,682', '2,868', '1,845', '6,395']
        ]
      ),
      age: 120,
      folder: 0,
      kind: 'sheet',
      tags: ['spreadsheet', 'table', 'budget', 'numbers', 'excel'],
      colors: ['#ffffff', '#217346', '#f3f3f3']
    },
    {
      c: code(
        'tokens.css',
        [
          ':root {',
          '  --ink-900: #111014;',
          '  --ink-700: #2a2730;',
          '  --paper: #f7f3ea;',
          '  --amber: #f2b544;',
          '  --ember: #e0703a;',
          '  --radius: 8px;',
          '}',
          '',
          '.magpie {',
          '  box-shadow: 0 0 0 1px var(--amber);',
          '}'
        ],
        TREE
      ),
      age: 130,
      folder: 0,
      kind: 'code',
      tags: [...codeTags, 'css'],
      colors: VS
    },
    // this month
    {
      c: slack('general', [
        ['Ana Ruiz', '9:02 AM', 'heads up: design review moved to 3:30 pm tomorrow, same room'],
        ['Ben Cho', '9:05 AM', 'thanks! I will bring the onboarding flows'],
        ['Ana Ruiz', '9:06 AM', 'agenda: search results, settings, and the empty states']
      ]),
      age: 170,
      folder: 0,
      kind: 'chat',
      tags: [...chatTags, 'slack', 'meeting'],
      colors: SL
    },
    {
      c: code(
        'Tile.tsx',
        [
          "import type { SearchHit } from '../../shared/types'",
          '',
          'interface Props {',
          '  hit: SearchHit',
          '  selected: boolean',
          '}',
          '',
          'export function Tile({ hit, selected }: Props) {',
          '  const { shot, highlights } = hit',
          '  return (',
          '    <div className="tile" aria-selected={selected}>',
          '      <Thumb shot={shot} highlights={highlights} />',
          '      <p className="caption">{hit.snippet ?? shot.name}</p>',
          '    </div>',
          '  )',
          '}'
        ],
        TREE
      ),
      age: 200,
      folder: 0,
      kind: 'code',
      tags: [...codeTags, 'react'],
      colors: VS
    },
    {
      c: web(
        'https://en.wikipedia.org/wiki/Gyotaku',
        'Wikipedia',
        'Gyotaku',
        'From Wikipedia, the free encyclopedia',
        [
          'Gyotaku is a Japanese method of printing fish with ink on paper, practiced since the mid-1800s.',
          'Fishermen used it to record their catches. The fish is brushed with sumi ink and pressed onto rice paper.',
          'Today the prints are valued as art, and the technique is used to teach anatomy and conservation.'
        ]
      ),
      age: 230,
      folder: 0,
      kind: 'web',
      tags: [...webTags, 'wiki'],
      colors: WEB
    },
    {
      c: photo('beach'),
      age: 260,
      folder: 1,
      kind: 'photo',
      tags: ['beach', 'sea', 'ocean', 'summer', 'sand', 'vacation', 'blue'],
      colors: ['#8fd3f4', '#2a9fd6', '#f1d9a7'],
      name: 'IMG_1874.jpg'
    },
    {
      c: dialog(
        'python.exe',
        'Python has stopped working',
        [
          'A problem caused the program to stop working correctly.',
          'Faulting module: onnxruntime.dll, version 1.30.0'
        ],
        ['Close program']
      ),
      age: 300,
      folder: 0,
      kind: 'dialog',
      tags: dlgTags,
      colors: DLG
    },
    {
      c: invoice({
        vendor: 'Cloudbase Hosting',
        addr: ['410 Terry Ave N', 'Seattle, WA 98109'],
        heading: 'INVOICE',
        ocrHeading: 'INV0ICE',
        meta: ['Invoice no. CB-88213-09', 'Period: 1–30 Sep 2026', 'Account 4417-2290'],
        bill: ['Magpie Labs', 'billing@magpie.app'],
        items: [
          ['Compute (t4g.small, 720 hrs)', '1', '$12.10'],
          ['Object storage (412 GB)', '1', '$9.47'],
          ['Data transfer out', '1', '$61.55']
        ],
        totals: [
          ['Subtotal', '$83.12'],
          ['Tax', '$0.00'],
          ['Total', '$83.12']
        ],
        foot: ['Questions? support@cloudbase.dev', 'cloudbase.dev/billing']
      }),
      age: 330,
      folder: 0,
      kind: 'receipt',
      tags: recTags,
      colors: INV
    },
    {
      c: sms('AD-NOVABK', [
        [
          '14 Sep 1:20 PM',
          'Rs. 2,499.00 debited from A/c XX1234 on 14-09-26 to VPA rio@okbank. Not you? Call +91 18002 026161.'
        ]
      ]),
      age: 400,
      folder: 0,
      kind: 'sms',
      tags: [...smsTags, 'bank'],
      colors: SMS
    },
    {
      c: terminal('bash', [
        ['$ cargo build'],
        ['   Compiling magpie-ocr v0.3.1', '#23d18b'],
        ['error[E0382]: borrow of moved value: `query`', '#f14c4c'],
        ['  --> src/search.rs:42:18', '#3b8eea'],
        ['38 |     let query = normalize(input);'],
        ['   |         ----- move occurs because `query` has type `String`'],
        ['40 |     index.lookup(query);'],
        ['   |                  ----- value moved here'],
        ['42 |     println!("{}", query);'],
        ['   |                    ^^^^^ value borrowed here after move', '#f14c4c'],
        ['error: could not compile `magpie-ocr` due to 1 previous error', '#f14c4c']
      ]),
      age: 450,
      folder: 0,
      kind: 'terminal',
      tags: [...termTags, 'rust'],
      colors: TM
    },
    {
      c: terminal('bash', [
        ['$ docker build -t magpie-indexer .'],
        ['[+] Building 41.2s (9/12)'],
        [' => [3/7] RUN apt-get update && apt-get install -y tesseract-ocr'],
        [' => ERROR [5/7] RUN pip install -r requirements.txt', '#f14c4c'],
        ['------'],
        [' > [5/7] RUN pip install -r requirements.txt:'],
        ['ERROR: Could not find a version that satisfies the requirement torch==2.9.1', '#f14c4c'],
        ['ERROR: No matching distribution found for torch==2.9.1', '#f14c4c']
      ]),
      age: 500,
      folder: 0,
      kind: 'terminal',
      tags: [...termTags, 'docker'],
      colors: TM
    },
    {
      c: photo('cat'),
      age: 560,
      folder: 1,
      kind: 'photo',
      tags: ['cat', 'pet', 'animal', 'kitten', 'orange'],
      colors: ['#3c6e91', '#e8954a', '#2b506a'],
      name: 'IMG_1822.jpg',
      pinned: true
    },
    {
      c: web(
        'https://news.example.org/city/cycling-lanes',
        'City Ledger',
        'Council approves 40 km of new cycling lanes',
        'Construction starts in January',
        [
          'The city council voted 9 to 2 on Tuesday to fund protected cycling lanes along four major roads.',
          'The first segment, from the railway station to the tech park, is expected to open by June.',
          'Residents can comment on the routes until 30 October at cityledger.example.org/lanes.'
        ]
      ),
      age: 620,
      folder: 0,
      kind: 'web',
      tags: [...webTags, 'news'],
      colors: WEB
    },
    {
      c: discord('general', [
        [
          'priya',
          'Last Sunday at 4:12 PM',
          'meetup is at 221B Baker Street, London NW1 6XE. doors open 6:30'
        ],
        ['alex', 'Last Sunday at 4:15 PM', 'is there parking nearby?'],
        ['priya', 'Last Sunday at 4:16 PM', 'there is a garage on Melcombe St, about 3 min walk']
      ]),
      age: 680,
      folder: 2,
      kind: 'chat',
      tags: [...chatTags, 'discord', 'address'],
      colors: DC
    },
    // older
    {
      c: photo('dog'),
      age: 1000,
      folder: 1,
      kind: 'photo',
      tags: ['dog', 'pet', 'animal', 'puppy', 'grass', 'green'],
      colors: ['#9cc56b', '#a8743f', '#6f9a45'],
      name: 'IMG_1701.jpg'
    },
    {
      c: web(
        'https://github.com/magpie-app/magpie/issues/412',
        'GitHub',
        'Thumbnails flicker when the window regains focus #412',
        'Open · opened by rio',
        [
          'Steps to reproduce: open Magpie with the hotkey, switch to another app, then switch back.',
          'Expected: thumbnails stay rendered. Actual: every tile fades out and back in.',
          'Version 0.9.2 on Windows 11 23H2, GPU: Intel Iris Xe.'
        ]
      ),
      age: 1300,
      folder: 0,
      kind: 'web',
      tags: [...webTags, 'github', 'issue', 'bug'],
      colors: WEB
    },
    {
      c: photo('pizza'),
      age: 1700,
      folder: 1,
      kind: 'photo',
      tags: ['pizza', 'food', 'dinner', 'meal', 'red'],
      colors: ['#7b5236', '#f0c260', '#b8322a'],
      name: 'IMG_1650.jpg'
    },
    {
      c: code(
        'scrape.py',
        [
          'import csv',
          'import requests',
          '',
          'API = "https://api.example.com/v1/screens"',
          '',
          'def fetch_page(page: int) -> list[dict]:',
          '    r = requests.get(API, params={"page": page}, timeout=10)',
          '    r.raise_for_status()',
          '    return r.json()["items"]',
          '',
          'with open("screens.csv", "w", newline="") as f:',
          '    writer = csv.writer(f)',
          '    for page in range(1, 6):',
          '        for item in fetch_page(page):',
          '            writer.writerow([item["id"], item["title"]])'
        ],
        ['scrape.py', 'requirements.txt', 'screens.csv']
      ),
      age: 2100,
      folder: 0,
      kind: 'code',
      tags: [...codeTags, 'python'],
      colors: VS
    },
    {
      c: invoice({
        vendor: 'Ridely',
        addr: ['Thanks for riding, Alex', 'Sat, 9 May 2026'],
        heading: 'Trip receipt',
        meta: ['Trip ID R-55120', '34.2 km · 52 min'],
        bill: ['Indiranagar, Bengaluru', 'to Kempegowda Intl Airport'],
        items: [
          ['Trip fare', '1', '₹412.00'],
          ['Airport toll', '1', '₹60.00'],
          ['Booking fee', '1', '₹14.20']
        ],
        totals: [
          ['Subtotal', '₹486.20'],
          ['Discount', '₹0.00'],
          ['Total', '₹486.20']
        ],
        foot: ['Lost something? help@ridely.example']
      }),
      age: 3600,
      folder: 0,
      kind: 'receipt',
      tags: [...recTags, 'taxi', 'ride', 'travel'],
      colors: INV
    },
    {
      c: photo('forest'),
      age: 2600,
      folder: 1,
      kind: 'photo',
      tags: ['forest', 'trees', 'green', 'nature', 'hiking', 'landscape'],
      colors: ['#cfe3d4', '#2f5d3a', '#3f7a4a'],
      name: 'IMG_1544.jpg'
    },
    {
      c: sms('AX-PARCEL', [
        [
          'Fri 11:02 AM',
          'Your parcel AZ-55120 is out for delivery today. Delivery code: 618204. Track at parcel.example/t/55120'
        ]
      ]),
      age: 3100,
      folder: 0,
      kind: 'sms',
      tags: [...smsTags, 'delivery'],
      colors: SMS
    },
    {
      c: discord('random', [
        [
          'kai',
          'Thursday at 7:02 PM',
          'dinner at Sakura on Friday? they finally have the omakase back'
        ],
        ['noor', 'Thursday at 7:05 PM', 'yes, 8pm works for me'],
        ['kai', 'Thursday at 7:11 PM', 'booked a table for 4, confirmation SKR-7781']
      ]),
      age: 4100,
      folder: 2,
      kind: 'chat',
      tags: [...chatTags, 'discord', 'dinner'],
      colors: DC
    },
    {
      c: palette('Brand v2', [
        ['Night', '#0f172a'],
        ['Signal', '#3b82f6'],
        ['Go', '#22c55e'],
        ['Stop', '#f43f5e']
      ]),
      age: 4600,
      folder: 0,
      kind: 'design',
      tags: ['palette', 'colors', 'design', 'brand', 'swatches'],
      colors: ['#1e1e1e', '#ffffff', '#3b82f6']
    },
    {
      c: sheet(
        'Monthly expenses.xlsx',
        ['Item', 'Mar', 'Apr', 'May', 'Total'],
        [
          ['Rent', '32,000', '32,000', '32,000', '96,000'],
          ['Groceries', '8,450', '7,920', '9,105', '25,475'],
          ['Internet', '1,199', '1,199', '1,199', '3,597'],
          ['Total', '41,649', '41,119', '42,304', '125,072']
        ]
      ),
      age: 5200,
      folder: 0,
      kind: 'sheet',
      tags: ['spreadsheet', 'table', 'budget', 'numbers', 'expenses'],
      colors: ['#ffffff', '#217346', '#f3f3f3']
    },
    {
      c: dialog(
        'Windows Update',
        'Restart required',
        [
          'Updates are ready to install. Your device will restart',
          'outside of active hours (8:00 AM to 5:00 PM).'
        ],
        ['Restart now', 'Schedule'],
        true
      ),
      age: 5800,
      folder: 0,
      kind: 'dialog',
      tags: [...dlgTags, 'update'],
      colors: ['#24435f', '#f3f3f3', '#f7b500']
    },
    {
      c: terminal('bash', [
        ['$ ssh deploy@203.0.113.24'],
        ['deploy@203.0.113.24: Permission denied (publickey).', '#f14c4c'],
        ['$ ssh -i ~/.ssh/magpie_ed25519 deploy@203.0.113.24'],
        ['Welcome to Ubuntu 24.04.3 LTS'],
        ['Last login: Tue Mar 10 09:14:52 2026 from 198.51.100.7'],
        ['deploy@magpie-prod:~$']
      ]),
      age: 6300,
      folder: 0,
      kind: 'terminal',
      tags: [...termTags, 'ssh', 'server'],
      colors: TM
    },
    {
      c: web(
        'https://developer.mozilla.org/en-US/docs/Web/CSS/text-wrap',
        'MDN',
        'text-wrap',
        'CSS property',
        [
          'The text-wrap shorthand controls how text inside an element is wrapped.',
          'balance: wraps text so that each line has roughly the same length. Best for headings.',
          'pretty: avoids short last lines at the cost of slower layout. Best for body text.'
        ]
      ),
      age: 7000,
      folder: 0,
      kind: 'web',
      tags: [...webTags, 'docs', 'css'],
      colors: WEB
    },
    {
      c: slack('planning', [
        ['Ana Ruiz', '4:40 PM', 'Q4 planning doc is up: https://docs.northwind.dev/q4-planning'],
        ['Ben Cho', '4:52 PM', 'left comments on the search section, mostly about filters']
      ]),
      age: 7600,
      folder: 0,
      kind: 'chat',
      tags: [...chatTags, 'slack', 'planning'],
      colors: SL
    },
    {
      c: mapCard('Kempegowda Intl Airport', [
        'Terminal 2 · Departures',
        '36 min by car via NH 44',
        'Open 24 hours',
        'Directions     Call     Save'
      ]),
      age: 8000,
      folder: 0,
      kind: 'map',
      tags: ['map', 'location', 'airport', 'travel', 'directions'],
      colors: ['#e8eaed', '#ffffff', '#aadaff']
    },
    {
      c: photo('flowers'),
      age: 8400,
      folder: 1,
      kind: 'photo',
      tags: ['flower', 'flowers', 'garden', 'pink', 'red', 'nature'],
      colors: ['#2f4a2c', '#d6336c', '#f06595'],
      name: 'IMG_1402.jpg'
    }
  )
  return d
}

// ---------------------------------------------------------------- records

interface Rec {
  shot: Shot
  metas: LineMeta[]
  text: string
  tags: string[]
  kind: string
  actions: SmartAction[]
}

const pad = (n: number): string => String(n).padStart(2, '0')

function shotName(t: number): string {
  const d = new Date(t)
  return `Screenshot ${d.getFullYear()}-${pad(d.getMonth() + 1)}-${pad(d.getDate())} ${pad(d.getHours())}${pad(d.getMinutes())}${pad(d.getSeconds())}.png`
}

function findActions(text: string): SmartAction[] {
  const out: SmartAction[] = []
  const seen = new Set<string>()
  const add = (kind: SmartAction['kind'], value: string): void => {
    if (seen.has(kind + value)) return
    seen.add(kind + value)
    out.push({ kind, value })
  }
  const emails = text.match(/[\w.+-]+@[\w-]+\.[a-z.]{2,}/gi) ?? []
  emails.forEach((e) => add('email', e))
  const rest = emails.reduce((t, e) => t.replace(e, ' '), text)
  for (const u of rest.match(
    /\bhttps?:\/\/[^\s]+|\b(?:[a-z0-9-]+\.)+(?:com|io|org|dev|app|example)(?:\/[^\s]*)?/gi
  ) ?? [])
    add('url', u.replace(/[.,)]+$/, ''))
  for (const p of text.match(/\+\d{1,3}\s?(?:\(\d{3}\)\s?\d{3}-\d{4}|\d{5}\s\d{6})/g) ?? [])
    add('phone', p)
  if (/\b(code|otp|pin)\b/i.test(text)) {
    const m = text.slice(text.search(/\b(code|otp|pin)\b/i)).match(/\b(?!20\d\d\b)\d{4,6}\b/)
    if (m) add('code', m[0])
  }
  for (const c of text.match(/#[0-9a-f]{6}\b/gi) ?? []) add('color', c.toLowerCase())
  return out
}

/** `copies` > 1 repeats the library further back in time, for testing large grids (?copies=10). */
function build(copies = 1): Rec[] {
  const now = Date.now()
  const groupIds = new Map<string, number>()
  const list = Array.from({ length: copies }, (_, k) =>
    defs().map((d) => ({ ...d, age: d.age + k * 9000, group: d.group && `${d.group}${k}` }))
  )
    .flat()
    .map((d) => ({ d, mtime: Math.round(now - d.age * 3600_000) }))
    .sort((a, b) => a.mtime - b.mtime)
  const recs = list.map(({ d, mtime }, i): Rec => {
    const id = i + 1
    const url = d.c.url()
    const text = d.c.lines.map((m) => m.line.t).join('\n')
    if (d.group) groupIds.set(d.group, id) // ascending ids, so the last one wins: the newest
    const folder = FOLDERS[d.folder]
    const name =
      d.name ?? (d.folder === 2 ? `discord-${new Date(mtime).getDate()}${id}.png` : shotName(mtime))
    return {
      shot: {
        id,
        path: `${folder}\\${name}`,
        name,
        folder,
        mtime,
        size: Math.round(d.c.w * d.c.h * 0.21 + (hash(text) % 40000)),
        width: d.c.w,
        height: d.c.h,
        thumb: url,
        src: url,
        groupId: null,
        groupSize: 1,
        pinned: !!d.pinned,
        colors: d.colors,
        indexed: true
      },
      metas: d.c.lines,
      text,
      tags: d.kind === 'photo' ? d.tags : [d.kind, ...d.tags],
      kind: d.kind,
      actions: findActions(text)
    }
  })
  list.forEach(({ d }, i) => {
    if (d.group) recs[i].shot.groupId = groupIds.get(d.group)!
  })
  // A few new arrivals Magpie has not read yet: found by name and path only, no size or colors.
  const loose = recs.filter((r) => r.shot.groupId === null).reverse()
  UNREAD.forEach(([name, folder], i) => {
    const r = loose[[1, 4, 8, 13, 19][i]]
    if (!r) return
    const path = `${folder}\\${name}`
    Object.assign(r.shot, { name, folder, path, indexed: false, width: 0, height: 0, colors: [] })
    Object.assign(r, { metas: [], text: '', tags: [], kind: 'file', actions: [] })
  })
  return recs
}

const UNREAD: [name: string, folder: string][] = [
  ['IMG_4821.JPG', 'C:\\Users\\Alex\\Pictures\\Phone backup 2023\\DCIM\\Camera'],
  ['holiday-beach.jpg', 'C:\\Users\\Alex\\Pictures'],
  ['diagram-final-v3.png', 'C:\\Users\\Alex\\Desktop'],
  ['logo@2x.png', 'C:\\Users\\Alex\\Documents\\Projects\\magpie-site\\public\\images'],
  ['Untitled.png', 'C:\\Users\\Alex\\Pictures\\Screenshots']
]

// ---------------------------------------------------------------- query parsing

const MONTHS = ['jan', 'feb', 'mar', 'apr', 'may', 'jun', 'jul', 'aug', 'sep', 'oct', 'nov', 'dec']
const fmtDay = new Intl.DateTimeFormat(undefined, {
  day: 'numeric',
  month: 'short',
  year: 'numeric'
})
const fmtMonth = new Intl.DateTimeFormat(undefined, { month: 'short', year: 'numeric' })

function parseDate(v: string): { t: number; label: string } | null {
  const m = MONTHS.indexOf(v.slice(0, 3))
  if (m >= 0) {
    const now = new Date()
    const y = m > now.getMonth() ? now.getFullYear() - 1 : now.getFullYear()
    const t = new Date(y, m, 1).getTime()
    return { t, label: fmtMonth.format(t) }
  }
  const t = Date.parse(v)
  return Number.isNaN(t) ? null : { t, label: fmtDay.format(t) }
}

function hue(hex: string): string {
  const n = parseInt(hex.slice(1), 16)
  const r = (n >> 16) / 255
  const g = ((n >> 8) & 255) / 255
  const b = (n & 255) / 255
  const max = Math.max(r, g, b)
  const min = Math.min(r, g, b)
  const l = (max + min) / 2
  const s = max === min ? 0 : (max - min) / (1 - Math.abs(2 * l - 1))
  if (l < 0.14) return 'black'
  if (l > 0.92) return 'white'
  if (s < 0.18) return 'gray'
  let h =
    max === r
      ? ((g - b) / (max - min)) % 6
      : max === g
        ? (b - r) / (max - min) + 2
        : (r - g) / (max - min) + 4
  h = (h * 60 + 360) % 360
  if (h < 15 || h >= 345) return 'red'
  if (h < 42) return 'orange'
  if (h < 68) return 'yellow'
  if (h < 165) return 'green'
  if (h < 255) return 'blue'
  if (h < 290) return 'purple'
  return 'pink'
}

const HAS: Record<string, [SmartAction['kind'], string]> = {
  url: ['url', 'Has a link'],
  link: ['url', 'Has a link'],
  email: ['email', 'Has an email'],
  phone: ['phone', 'Has a phone number'],
  code: ['code', 'Has a code'],
  otp: ['code', 'Has a code'],
  color: ['color', 'Has a color value']
}

interface Parsed {
  terms: string[]
  filters: ActiveFilter[]
  preds: ((r: Rec) => boolean)[]
  sort?: SortOrder
}

const UNIT: Record<string, number> = { '': 1, b: 1, kb: 1024, mb: 1024 ** 2, gb: 1024 ** 3 }
/** Everything's size buckets. */
const BUCKET: Record<string, [number, number]> = {
  tiny: [0, 10 * 1024],
  small: [10 * 1024, 100 * 1024],
  medium: [100 * 1024, 1024 ** 2],
  large: [1024 ** 2, 16 * 1024 ** 2],
  huge: [16 * 1024 ** 2, Infinity]
}

/** `>1mb`, `<=800`, `1mb..5mb`, `1920`, or (with units) a bucket like `large`. */
function compare(v: string, units: boolean): ((n: number) => boolean) | null {
  const amount = (s: string): number => {
    const m = /^(\d+(?:\.\d+)?)([kmg]?b)?$/.exec(s)
    return m && (units || !m[2]) ? Number(m[1]) * UNIT[m[2] ?? ''] : NaN
  }
  const b = units ? BUCKET[v] : undefined
  if (b) return (n) => n >= b[0] && n < b[1]
  const range = v.split('..')
  if (range.length === 2) {
    const [lo, hi] = range.map(amount)
    return Number.isNaN(lo + hi) ? null : (n) => n >= lo && n <= hi
  }
  const m = /^(>=|<=|>|<|=)?(.+)$/.exec(v)
  const x = amount(m?.[2] ?? '')
  if (!m || Number.isNaN(x)) return null
  const ops: Record<string, (n: number) => boolean> = {
    '>': (n) => n > x,
    '<': (n) => n < x,
    '>=': (n) => n >= x,
    '<=': (n) => n <= x,
    '=': (n) => n === x
  }
  return ops[m[1] ?? '=']
}

const SORT_LABEL: Record<SortOrder, string> = {
  relevance: 'Best match first',
  newest: 'Newest first',
  oldest: 'Oldest first',
  largest: 'Largest first',
  smallest: 'Smallest first',
  name: 'By name'
}
const SORT_CMP: Record<SortOrder, (a: Shot, b: Shot) => number> = {
  relevance: () => 0,
  newest: (a, b) => b.mtime - a.mtime,
  oldest: (a, b) => a.mtime - b.mtime,
  largest: (a, b) => b.size - a.size,
  smallest: (a, b) => a.size - b.size,
  name: (a, b) => a.name.localeCompare(b.name, undefined, { numeric: true, sensitivity: 'base' })
}

function parse(q: string): Parsed {
  const out: Parsed = { terms: [], filters: [], preds: [] }
  const now = new Date()
  const today = new Date(now.getFullYear(), now.getMonth(), now.getDate()).getTime()
  const DAY = 86400_000
  const re =
    /(-?)(?:(in|date|dm|before|after|color|has|is|ext|size|path|width|height|sort):("[^"]*"|\S+)|"([^"]+)"|(\S+))/gi
  for (const m of q.matchAll(re)) {
    const neg = m[1] === '-'
    const raw = m[2]?.toLowerCase()
    const key = (raw === 'dm' ? 'date' : raw) as ActiveFilter['key'] | undefined
    const val = (m[3] ?? m[4] ?? m[5] ?? '').replace(/^"|"$/g, '').toLowerCase()
    if (!val) continue
    const f = (label: string, pred?: (r: Rec) => boolean): void => {
      out.filters.push({ key: key ?? 'not', value: val, label })
      if (pred) out.preds.push(pred)
    }
    if (!key) {
      if (neg) f(`Without “${val}”`, (r) => !(r.text + r.shot.name).toLowerCase().includes(val))
      else out.terms.push(val)
      continue
    }
    if (key === 'in') f(`Folder: ${val}`, (r) => r.shot.folder.toLowerCase().includes(val))
    else if (key === 'path') f(`Path: ${val}`, (r) => r.shot.path.toLowerCase().includes(val))
    else if (key === 'ext') {
      const exts = val.split(';').map((e) => e.replace(/^\./, ''))
      const ext = (r: Rec): string => r.shot.name.split('.').pop()!.toLowerCase()
      f(`Type: ${exts.join(', ').toUpperCase()}`, (r) => exts.includes(ext(r)))
    } else if (key === 'size' || key === 'width' || key === 'height') {
      const test = compare(val, key === 'size')
      const name = key[0].toUpperCase() + key.slice(1)
      // Unread images have no width or height yet, so they never match those.
      if (!test) f(`${name}: ${val} (not understood)`)
      else if (key === 'size') f(`Size: ${val}`, (r) => test(r.shot.size))
      else f(`${name}: ${val}`, (r) => r.shot.indexed && test(r.shot[key]))
    } else if (key === 'sort') {
      const s = Object.keys(SORT_LABEL).find((k) => k === val) as SortOrder | undefined
      if (s) out.sort = s
      f(s ? SORT_LABEL[s] : `Sort: ${val} (not understood)`)
    } else if (key === 'date') {
      const ranges: Record<string, [number, number, string]> = {
        today: [today, Infinity, 'Today'],
        yesterday: [today - DAY, today, 'Yesterday'],
        week: [today - 6 * DAY, Infinity, 'Last 7 days'],
        month: [today - 29 * DAY, Infinity, 'Last 30 days'],
        year: [today - 364 * DAY, Infinity, 'Last 12 months']
      }
      const r = ranges[val]
      if (r) f(r[2], (x) => x.shot.mtime >= r[0] && x.shot.mtime < r[1])
      else f(`Date: ${val} (not understood)`)
    } else if (key === 'before' || key === 'after') {
      const d = parseDate(val)
      if (!d) f(`${key === 'before' ? 'Before' : 'After'}: ${val} (not understood)`)
      else if (key === 'before') f(`Before ${d.label}`, (r) => r.shot.mtime < d.t)
      else f(`After ${d.label}`, (r) => r.shot.mtime >= d.t)
    } else if (key === 'color')
      f(`Color: ${val}`, (r) => r.shot.colors.slice(0, 3).some((c) => hue(c) === val))
    else if (key === 'has') {
      const h = HAS[val]
      if (h) f(h[1], (r) => r.actions.some((a) => a.kind === h[0]))
      else f(`Has: ${val} (not understood)`)
    } else if (key === 'is') {
      if (val === 'pinned') f('Pinned', (r) => r.shot.pinned)
      else if (val === 'landscape')
        f('Landscape', (r) => r.shot.indexed && r.shot.width > r.shot.height)
      else if (val === 'portrait')
        f('Portrait', (r) => r.shot.indexed && r.shot.height > r.shot.width)
      else f(`Is: ${val} (not understood)`)
    }
  }
  return out
}

/** Folds OCR look-alikes: 0/O, 1/l/I, rn/m. */
const fold = (s: string): string =>
  s
    .toLowerCase()
    .replace(/rn/g, 'm')
    .replace(/0/g, 'o')
    .replace(/[1|!i]/g, 'l')

/** Sub-box of characters [a, b) inside an OCR line, using the advances it was drawn with. */
function subBox(m: LineMeta, a: number, b: number): OcrLine {
  const { line } = m
  const total = textW(line.t, m.size, m.mono, m.bold)
  const pre = textW(line.t.slice(0, a), m.size, m.mono, m.bold)
  const mid = textW(line.t.slice(a, b), m.size, m.mono, m.bold)
  return {
    t: line.t.slice(a, b),
    x: line.x + (pre / total) * line.w,
    y: line.y,
    w: (mid / total) * line.w,
    h: line.h
  }
}

// ---------------------------------------------------------------- the API

function emitter<T>(): { on(cb: (v: T) => void): () => void; emit(v: T): void } {
  const subs = new Set<(v: T) => void>()
  return {
    on(cb) {
      subs.add(cb)
      return () => {
        subs.delete(cb)
      }
    },
    emit(v) {
      subs.forEach((cb) => cb(v))
    }
  }
}

const sleep = (ms: number): Promise<void> => new Promise((r) => setTimeout(r, ms))

export function createMockApi(): MagpieApi {
  const params = new URLSearchParams(location.search)
  const platform = (params.get('platform') as MagpieApi['platform'] | null) ?? 'win32'
  const recs = build(Math.max(1, Number(params.get('copies')) || 1))
  const byId = new Map(recs.map((r) => [r.shot.id, r]))
  const hidden = new Set<number>()
  let indexCleared = false
  const metadata = new Map<number, ShotMetadata>()
  const rejected = new Set<string>()
  let pending: { ids: number[]; timer: number } | null = null
  let settings: Settings = {
    excludedFolders: [],
    savedSearches: [],
    folders: params.get('onboarding') === '1' ? [] : FOLDERS.slice(0, 3),
    hotkey: 'Alt+Shift+S',
    copyLatestHotkey: 'Alt+Shift+V',
    launchAtLogin: true,
    semantic: true,
    theme: 'system',
    hideOnBlur: true,
    onboarded: params.get('onboarding') !== '1',
    scope: 'folders',
    sharpText: true,
    saveClipboard: false
  }
  const status: IndexStatus = {
    forceIndexing: false,
    warnings: [],
    state: 'indexing',
    total: recs.length,
    ocrDone: recs.length - 9,
    embedded: recs.length - 21,
    errors: 1,
    model:
      params.get('model') === 'ready'
        ? { state: 'ready' }
        : { state: 'downloading', progress: 0.74 },
    sharp: recs.length - 40,
    // ?textModel=error shows the failure path.
    textModel:
      params.get('textModel') === 'error'
        ? { state: 'error', error: 'Couldn’t download the text model. Check your connection.' }
        : { state: 'downloading', progress: 0.4 }
  }
  const statusE = emitter<IndexStatus>()
  const indexedE = emitter<void>()
  const shownE = emitter<void>()
  const settingsE = emitter<void>()
  let ticks = 0
  const copy = (): IndexStatus => ({
    ...status,
    model: { ...status.model },
    textModel: { ...status.textModel }
  })

  const tick = {
    timer: 0,
    start() {
      if (!this.timer) this.timer = window.setInterval(step, 700)
    },
    stop() {
      clearInterval(this.timer)
      this.timer = 0
    }
  }
  function step(): void {
    if (status.state === 'paused') return tick.stop()
    const before = JSON.stringify(status)
    const m = status.model
    if (!settings.semantic) status.model = { state: 'off' }
    else if (m.state === 'off') status.model = { state: 'downloading', progress: 0 }
    else if (m.state === 'downloading') {
      const p = (m.progress ?? 0) + 0.05
      status.model = p >= 1 ? { state: 'loading' } : { state: 'downloading', progress: p }
    } else if (m.state === 'loading') status.model = { state: 'ready' }
    const t = status.textModel
    if (!settings.sharpText || platform === 'darwin') status.textModel = { state: 'off' }
    else if (t.state === 'off') status.textModel = { state: 'downloading', progress: 0 }
    else if (t.state === 'downloading') {
      const p = (t.progress ?? 0) + 0.1
      status.textModel = p >= 1 ? { state: 'loading' } : { state: 'downloading', progress: p }
    } else if (t.state === 'loading') status.textModel = { state: 'ready' }
    if (status.ocrDone < status.total) status.ocrDone++
    if (status.model.state === 'ready' && status.embedded < status.total) status.embedded++
    if (status.textModel.state === 'ready' && status.sharp < status.ocrDone) status.sharp += 2
    status.sharp = Math.min(status.sharp, status.ocrDone)
    const tm = status.textModel.state
    const sharpening =
      tm === 'downloading' || tm === 'loading' || (tm === 'ready' && status.sharp < status.ocrDone)
    const done =
      status.ocrDone >= status.total &&
      (!settings.semantic || status.embedded >= status.total) &&
      status.model.state !== 'downloading'
    status.state = done ? 'idle' : 'indexing'
    if (JSON.stringify(status) !== before) statusE.emit(copy())
    if (!done && ++ticks % 6 === 0) indexedE.emit()
    if (done && !sharpening) {
      indexedE.emit()
      tick.stop()
    }
  }
  tick.start()

  const visible = (): Rec[] => recs.filter((r) => !hidden.has(r.shot.id))
  const groupSize = (gid: number | null): number =>
    gid === null ? 1 : visible().filter((r) => r.shot.groupId === gid).length

  function hit(
    r: Rec,
    match: MatchKind,
    score: number,
    highlights: OcrLine[] = [],
    snippet?: string
  ): SearchHit {
    return { shot: { ...r.shot }, match, score, highlights, snippet, evidence: [match] }
  }

  /** Keeps the best hit of each burst and stamps it with the burst size. */
  function foldGroups(hits: SearchHit[]): SearchHit[] {
    const seen = new Set<number>()
    return hits.filter((h) => {
      const g = h.shot.groupId
      if (g === null) return true
      if (seen.has(g)) return false
      seen.add(g)
      h.shot.groupSize = groupSize(g)
      return true
    })
  }

  /** Every term in the OCR text or name (exactly, or after folding look-alikes), with boxes. */
  function textMatch(
    r: Rec,
    terms: string[]
  ): { match: 'text' | 'near'; highlights: OcrLine[]; snippet?: string } | null {
    // Unread images are found by name and path only.
    const hay = (r.text + '\n' + (r.shot.indexed ? r.shot.name : r.shot.path)).toLowerCase()
    const exact = terms.every((t) => hay.includes(t))
    if (!exact && !terms.every((t) => fold(hay).includes(fold(t)))) return null
    const highlights: OcrLine[] = []
    let snippet: string | undefined
    for (const m of r.metas) {
      const line = exact ? m.line.t.toLowerCase() : fold(m.line.t)
      for (const t of terms) {
        const needle = exact ? t : fold(t)
        for (let i = line.indexOf(needle); i >= 0; i = line.indexOf(needle, i + needle.length)) {
          highlights.push(subBox(m, i, i + needle.length))
          snippet ??= m.line.t
        }
      }
    }
    return { match: exact ? 'text' : 'near', highlights, snippet }
  }

  let activeSearches = 0
  async function search(req: SearchRequest): Promise<SearchResponse> {
    activeSearches++
    if (params.get('trace') === '1')
      console.info(
        '[mock] search',
        JSON.stringify({
          query: req.q,
          mode: req.mode,
          offset: req.offset ?? 0,
          active: activeSearches
        })
      )
    await sleep(Number(params.get('searchDelay')) || 8 + Math.random() * 40)
    activeSearches--
    if (params.get('failQuery') && req.q === params.get('failQuery'))
      throw new Error('Preview search failure. Retry or change the query.')
    const t0 = performance.now()
    const p = parse(req.q.replace(/(?:^|\s)(tag|collection):(?:"[^"]*"|\S+)/gi, ' '))
    const pool = visible().filter(
      (r) =>
        p.preds.every((f) => f(r)) &&
        !settings.excludedFolders.some((f) =>
          r.shot.path.toLowerCase().startsWith(f.toLowerCase() + '\\')
        ) &&
        [...req.q.matchAll(/(?:^|\s)(tag|collection):("[^"]*"|\S+)/gi)].every((m) => {
          const data = metadata.get(r.shot.id)
          return (m[1].toLowerCase() === 'tag' ? data?.tags : data?.collections)?.some(
            (v) => v.toLowerCase() === m[2].replace(/^"|"$/g, '').toLowerCase()
          )
        })
    )
    const semantic = settings.semantic ? status.model.state : 'off'
    let hits: SearchHit[] = []
    let textCount = 0

    if (req.expandGroup !== undefined) {
      hits = pool
        .filter((r) => r.shot.groupId === req.expandGroup)
        .sort((a, b) => b.shot.mtime - a.shot.mtime)
        .map((r) => hit(r, 'recent', 0))
    } else if (req.similarTo !== undefined || req.imagePath || req.imageData) {
      // Hybrid: the image ranks by look, words boost (and light up) shots that also contain them.
      const ref = req.similarTo !== undefined ? byId.get(req.similarTo) : undefined
      const refTags = ref ? ref.tags : ['photo', 'landscape', 'sky', 'blue', 'green', 'orange']
      hits = foldGroups(
        pool
          .filter(
            (r) =>
              r !== ref && (!ref || r.shot.groupId === null || r.shot.groupId !== ref.shot.groupId)
          )
          .map((r) => {
            const overlap = r.tags.filter((t) => refTags.includes(t)).length
            const same = ref && r.kind === ref.kind ? 3 : 0
            const seed = req.imagePath ?? String(req.imageData?.length ?? '')
            const tm = p.terms.length ? textMatch(r, p.terms) : null
            const tagged = p.terms.filter((t) => r.tags.some((g) => g.startsWith(t))).length
            const score =
              overlap + same + (hash(r.text + seed) % 10) / 20 + (tm ? 6 : 0) + tagged * 3
            return { r, score, tm, words: !p.terms.length || !!tm || tagged > 0 }
          })
          .filter((x) => x.score >= 1 && x.words)
          .sort((a, b) => b.score - a.score)
          .map((x) =>
            x.tm
              ? hit(x.r, x.tm.match, x.score / 10, x.tm.highlights, x.tm.snippet)
              : hit(x.r, 'similar', x.score / 10)
          )
      )
    } else if (!p.terms.length) {
      const list = [...pool].sort((a, b) => b.shot.mtime - a.shot.mtime)
      if (req.shuffle) list.sort(() => Math.random() - 0.5)
      hits = foldGroups(list.map((r) => hit(r, 'recent', 0)))
    } else {
      const textHits: SearchHit[] = []
      for (const r of pool) {
        const tm = textMatch(r, p.terms)
        if (tm)
          textHits.push(
            hit(
              r,
              tm.match,
              tm.highlights.length + (tm.match === 'text' ? 10 : 0),
              tm.highlights,
              tm.snippet
            )
          )
      }
      if (req.mode === 'visual') textHits.length = 0
      textHits.sort((a, b) => b.score - a.score || b.shot.mtime - a.shot.mtime)
      textCount = textHits.length
      const taken = new Set(textHits.map((h) => h.shot.id))
      const visual =
        semantic === 'ready' && req.mode !== 'text'
          ? pool
              .filter((r) => !taken.has(r.shot.id))
              .map((r) => ({
                r,
                score: p.terms.filter((t) =>
                  r.tags.some((g) => g === t || g.startsWith(t) || t.startsWith(g))
                ).length
              }))
              .filter((x) => x.score > 0)
              .sort((a, b) => b.score - a.score || b.r.shot.mtime - a.r.shot.mtime)
              .map((x) => hit(x.r, 'visual', 0.2 + x.score / 10))
          : []
      hits = foldGroups([...textHits, ...visual])
    }
    const order = p.sort ?? req.sort
    if (order && !req.shuffle) hits.sort((a, b) => SORT_CMP[order](a.shot, b.shot))
    hits = hits.filter((h) => !rejected.has(`${req.q.trim().toLowerCase()}:${h.shot.id}`))
    const offset = req.offset ?? 0
    const limit = req.limit ?? 120
    return {
      hits: hits.slice(offset, offset + limit),
      hasMore: offset + limit < hits.length,
      nextOffset: offset + limit < hits.length ? offset + limit : undefined,
      query: req.q,
      mode: req.mode ?? 'all',
      textCount,
      tookMs: Math.max(1, Math.round(performance.now() - t0 + 2)),
      filters: p.filters,
      semantic
    }
  }

  const stats = async (): Promise<Stats> => {
    const v = visible()
    const now = new Date()
    const months = Array.from({ length: 12 }, (_, i) => {
      const d = new Date(now.getFullYear(), now.getMonth() - 11 + i, 1)
      const key = `${d.getFullYear()}-${pad(d.getMonth() + 1)}`
      return {
        month: key,
        count: v.filter((r) => {
          const m = new Date(r.shot.mtime)
          return `${m.getFullYear()}-${pad(m.getMonth() + 1)}` === key
        }).length
      }
    })
    const colorCount = new Map<string, number>()
    v.forEach((r) => {
      const c =
        r.shot.colors.find((h) => !['black', 'white', 'gray'].includes(hue(h))) ?? r.shot.colors[0]
      colorCount.set(c, (colorCount.get(c) ?? 0) + 1)
    })
    return {
      total: v.length,
      bytes: v.reduce((n, r) => n + r.shot.size, 0),
      folders: settings.folders.map((f) => ({
        path: f,
        count: v.filter((r) => r.shot.folder.startsWith(f)).length
      })),
      months,
      colors: [...colorCount]
        .map(([hex, count]) => ({ hex, count }))
        .sort((a, b) => b.count - a.count)
        .slice(0, 8),
      withText: v.filter((r) => r.metas.length > 0).length
    }
  }

  let picks = 0
  const api: MagpieApi = {
    platform,
    search,
    async getShot(id) {
      await sleep(10)
      const r = byId.get(id)
      if (!r || hidden.has(id)) return null
      const d: ShotDetail = {
        ...r.shot,
        groupSize: groupSize(r.shot.groupId),
        text: r.text,
        lines: r.metas.map((m) => m.line),
        actions: r.actions,
        metadata: metadata.get(id) ?? { note: '', tags: [], collections: [], sourceUrl: '' }
      }
      return d
    },
    status: async () => copy(),
    stats,
    getSettings: async () => ({ ...settings }),
    async setSettings(patch) {
      // Pretend the OS already owns these, so the "taken" path can be tried in the browser.
      const taken = ['Control+Space', 'Alt+Space', 'Super+Space', 'Command+Space']
      const next = { ...settings, ...patch }
      if (taken.includes(next.hotkey)) next.hotkey = settings.hotkey
      if (taken.includes(next.copyLatestHotkey)) next.copyLatestHotkey = settings.copyLatestHotkey
      settings = next
      if (
        indexCleared &&
        (settings.folders.length || settings.scope === 'everywhere' || settings.saveClipboard)
      ) {
        hidden.clear()
        indexCleared = false
        status.total = recs.length
        status.state = 'indexing'
        statusE.emit(copy())
        indexedE.emit()
      }
      if (patch.semantic !== undefined || patch.sharpText !== undefined || patch.folders)
        tick.start()
      return { ...settings }
    },
    async suggestFolders(): Promise<FolderSuggestion[]> {
      await sleep(300)
      return [
        { path: FOLDERS[0], count: 1204, label: 'Windows screenshots' },
        { path: FOLDERS[1], count: 86, label: 'Desktop' },
        { path: FOLDERS[2], count: 342, label: 'Saved from Discord' },
        { path: 'C:\\Users\\Alex\\Downloads', count: 0, label: 'Downloads' }
      ]
    },
    async pickFolder() {
      await sleep(200)
      return `C:\\Users\\Alex\\Documents\\Captures${picks++ ? ` ${picks}` : ''}`
    },
    async copyText(text) {
      await navigator.clipboard?.writeText(text).catch(() => undefined)
    },
    async copyImage(id) {
      console.info('[mock] copyImage', id)
    },
    async open(id) {
      console.info('[mock] open', id)
    },
    async reveal(id) {
      console.info('[mock] reveal', id)
    },
    async trash(ids) {
      if (pending) clearTimeout(pending.timer)
      ids.forEach((id) => hidden.add(id))
      pending = { ids, timer: window.setTimeout(() => (pending = null), 8000) }
    },
    async undoTrash() {
      if (!pending) return 0
      clearTimeout(pending.timer)
      pending.ids.forEach((id) => hidden.delete(id))
      const n = pending.ids.length
      pending = null
      return n
    },
    async pin(id, pinned) {
      const r = byId.get(id)
      if (r) r.shot.pinned = pinned
    },
    async reindex() {
      status.ocrDone = 0
      status.embedded = 0
      status.sharp = 0
      status.state = 'indexing'
      tick.start()
    },
    async pause(paused) {
      status.state = paused ? 'paused' : 'indexing'
      statusE.emit(copy())
      if (!paused) tick.start()
    },
    async finishIndexing(enabled) {
      status.forceIndexing = enabled
      status.waitingReason = undefined
      status.state = 'indexing'
      statusE.emit(copy())
      tick.start()
    },
    async retryFailed() {
      status.errors = 0
      statusE.emit(copy())
    },
    async repairModels() {
      status.model = { state: 'loading' }
      tick.start()
    },
    async failures() {
      return status.errors
        ? [
            {
              id: 1,
              name: 'Unavailable image.png',
              error: 'Could not decode image. Check the original file and retry.'
            }
          ]
        : []
    },
    async updateMetadata(id, value) {
      metadata.set(id, value)
      return value
    },
    async setRelevant(id, query, relevant) {
      const key = `${query.trim().toLowerCase()}:${id}`
      if (relevant) rejected.delete(key)
      else rejected.add(key)
    },
    async exportShots() {
      throw new Error('Export requires the desktop app. Browser preview uses sample files.')
    },
    async exportDiagnostics() {
      throw new Error('Diagnostic export requires the desktop app.')
    },
    async clearIndex() {
      settings = { ...settings, scope: 'folders', folders: [], saveClipboard: false }
      indexCleared = true
      tick.stop()
      hidden.clear()
      recs.forEach((r) => hidden.add(r.shot.id))
      status.total = status.ocrDone = status.embedded = status.sharp = 0
      metadata.clear()
      rejected.clear()
      recs.forEach((r) => {
        r.shot.pinned = false
      })
      status.errors = 0
      status.forceIndexing = false
      status.waitingReason = undefined
      status.state = 'idle'
      statusE.emit(copy())
      indexedE.emit()
    },
    async openExternal(url) {
      console.info('[mock] openExternal', url)
    },
    hide() {
      console.info('[mock] hide')
    },
    startDrag(id, path) {
      console.info('[mock] startDrag', id, path)
    },
    pathForFile: (file) => file.name,
    onStatus: statusE.on,
    onShown: shownE.on,
    onIndexed: indexedE.on,
    onOpenSettings: settingsE.on
  }
  // Handy for poking the mock from devtools: __magpie.shown(), __magpie.openSettings().
  Object.assign(window, {
    __magpie: { shown: () => shownE.emit(), openSettings: () => settingsE.emit() }
  })
  return api
}
