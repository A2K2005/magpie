// IPC contract shared by main, engine, preload and renderer.

/** One OCR line. Box coordinates are normalized (0–1), origin top-left. */
export interface OcrLine {
  t: string
  x: number
  y: number
  w: number
  h: number
}

export interface Shot {
  id: number
  path: string
  name: string
  /** Folder that directly contains the file. */
  folder: string
  /** Last-modified time, epoch ms. */
  mtime: number
  /** File size in bytes. */
  size: number
  width: number
  height: number
  /** `magpie://thumb/<id>` — 480px-wide WebP. */
  thumb: string
  /** `magpie://file/<id>` — the original image. */
  src: string
  /** Burst group id (id of the newest shot in the group), or null. */
  groupId: number | null
  /** Number of shots folded into this tile. 1 when not grouped. */
  groupSize: number
  pinned: boolean
  /** Dominant colors as hex, most dominant first. */
  colors: string[]
  /**
   * False until Magpie has read the image. Unread images are found by file name and path only;
   * width, height and colors are 0/empty, and the thumbnail is made on first request.
   */
  indexed: boolean
}

export type SmartActionKind = 'url' | 'email' | 'phone' | 'code' | 'color'

export interface SmartAction {
  kind: SmartActionKind
  value: string
}

export interface ShotDetail extends Shot {
  text: string
  lines: OcrLine[]
  actions: SmartAction[]
  metadata: ShotMetadata
}

export interface ShotMetadata {
  note: string
  tags: string[]
  collections: string[]
  sourceUrl: string
}

export type SearchMode = 'all' | 'text' | 'visual'
export interface SavedSearch {
  id: string
  name: string
  query: string
  mode: SearchMode
}

/**
 * Why a hit is in the results:
 * - text: every query word is a whole word in the OCR text or file name ("cred" in "CRED app")
 * - partial: every query word is there, but some only inside a longer word ("cred" in "Credila")
 * - near: matched only after folding OCR look-alikes (0/O, 1/l, rn/m)
 * - visual: CLIP thinks the image looks like the query
 * - similar: CLIP neighbour of a reference image
 * - recent: no query; newest first
 */
export type MatchKind = 'text' | 'partial' | 'near' | 'visual' | 'similar' | 'recent'

export interface SearchHit {
  evidence: string[]
  shot: Shot
  match: MatchKind
  score: number
  /** Boxes (normalized) to light up on the thumbnail. Empty for visual hits. */
  highlights: OcrLine[]
  /** The first OCR line that matched, for a caption. */
  snippet?: string
}

export interface SearchRequest {
  mode?: SearchMode
  offset?: number
  /** Raw query, including filters: `invoice in:discord date:week -draft color:red has:url is:pinned`. */
  q: string
  /** Find shots that look like this indexed shot. */
  similarTo?: number
  /** Find shots that look like this image file (dropped). */
  imagePath?: string
  /** Find shots that look like these image bytes (pasted from the clipboard). */
  imageData?: Uint8Array
  /** Show every member of this burst group instead of folding it. */
  expandGroup?: number
  /** Random sample instead of newest-first when the query is empty. */
  shuffle?: boolean
  /** Result order. Default: relevance for a query, newest first without one. `sort:` in the query wins. */
  sort?: SortOrder
  limit?: number
}

export type SortOrder = 'relevance' | 'newest' | 'oldest' | 'largest' | 'smallest' | 'name'

export interface ActiveFilter {
  key:
    | 'in'
    | 'date'
    | 'before'
    | 'after'
    | 'color'
    | 'has'
    | 'is'
    | 'not'
    | 'ext'
    | 'size'
    | 'path'
    | 'width'
    | 'height'
    | 'sort'
    | 'tag'
    | 'collection'
  value: string
  /** Human label, such as "Last 7 days" or "Folder: discord". */
  label: string
}

export type ModelState = 'off' | 'downloading' | 'loading' | 'ready' | 'error'

export interface SearchResponse {
  hasMore: boolean
  nextOffset?: number | null
  query: string
  mode: SearchMode
  hits: SearchHit[]
  /** Number of text + near matches before the limit was applied. */
  textCount: number
  tookMs: number
  filters: ActiveFilter[]
  /** Whether visual (CLIP) results could be included. */
  semantic: ModelState
  /** The query with misspelled words corrected ("saceage" → "savesage"), when that finds images. */
  didYouMean?: string
  /** The typed words matched nothing, so the hits shown are for `didYouMean`. */
  corrected?: boolean
}

export interface IndexStatus {
  waitingReason?: string | null
  forceIndexing: boolean
  warnings: string[]
  state: 'idle' | 'scanning' | 'indexing' | 'paused'
  total: number
  /** Shots with OCR done (searchable by text). */
  ocrDone: number
  /** Shots with a CLIP embedding (searchable by look). */
  embedded: number
  errors: number
  model: { state: ModelState; progress?: number; error?: string }
  /** Shots re-read by the accurate text model (Windows, `sharpText`). */
  sharp: number
  /** The accurate text model (PaddleOCR, ~13 MB). Always 'off' on macOS, where Apple Vision is already accurate. */
  textModel: { state: ModelState; progress?: number; error?: string }
}

export interface Settings {
  excludedFolders: string[]
  savedSearches: SavedSearch[]
  folders: string[]
  /** Electron accelerator, such as `Alt+Shift+S`. */
  hotkey: string
  /** Copies the newest screenshot's image to the clipboard from anywhere. Empty string = off. */
  copyLatestHotkey: string
  launchAtLogin: boolean
  /** Index and search by what images show (downloads the CLIP model once, ~150 MB). */
  semantic: boolean
  theme: 'system' | 'dark' | 'light'
  hideOnBlur: boolean
  onboarded: boolean
  /**
   * 'folders': only `folders`. 'everywhere': every image on the computer's fixed drives (Windows) or in
   * the home folder (macOS), plus `folders`; system, program and app-data folders are skipped.
   */
  scope: 'folders' | 'everywhere'
  /**
   * Windows only: after the fast built-in OCR, re-read text in the background with PaddleOCR
   * (more accurate; one ~13 MB download). Large backlogs wait until the user is away and on mains power.
   */
  sharpText: boolean
  /** Save images copied to the clipboard as PNGs in Pictures/Magpie Clipboard, so they become searchable. */
  saveClipboard: boolean
}

export interface FolderSuggestion {
  path: string
  /** Number of images directly or recursively inside. */
  count: number
  /** Short reason, such as "Windows screenshots". */
  label: string
}

export interface Stats {
  total: number
  bytes: number
  folders: { path: string; count: number }[]
  /** Last 12 months, oldest first. `month` is `YYYY-MM`. */
  months: { month: string; count: number }[]
  colors: { hex: string; count: number }[]
  withText: number
}

/** API exposed on `window.api` by the preload script. */
export interface MagpieApi {
  platform: 'darwin' | 'win32' | 'linux'
  search(req: SearchRequest): Promise<SearchResponse>
  getShot(id: number): Promise<ShotDetail | null>
  status(): Promise<IndexStatus>
  stats(): Promise<Stats>
  getSettings(): Promise<Settings>
  setSettings(patch: Partial<Settings>): Promise<Settings>
  suggestFolders(): Promise<FolderSuggestion[]>
  pickFolder(): Promise<string | null>
  copyText(text: string): Promise<void>
  copyImage(id: number): Promise<void>
  open(id: number): Promise<void>
  reveal(id: number): Promise<void>
  /** Hides the shots now; moves the files to the OS trash after 8 s unless undone. */
  trash(ids: number[]): Promise<void>
  /** Cancels the last pending trash. Returns how many shots came back. */
  undoTrash(): Promise<number>
  pin(id: number, pinned: boolean): Promise<void>
  reindex(): Promise<void>
  pause(paused: boolean): Promise<void>
  finishIndexing(enabled: boolean): Promise<void>
  retryFailed(): Promise<void>
  repairModels(): Promise<void>
  failures(): Promise<{ id: number; name: string; error: string }[]>
  updateMetadata(id: number, metadata: ShotMetadata): Promise<ShotMetadata>
  setRelevant(id: number, query: string, relevant: boolean): Promise<void>
  exportShots(ids: number[]): Promise<string | null>
  exportDiagnostics(): Promise<string | null>
  clearIndex(): Promise<void>
  openExternal(url: string): Promise<void>
  hide(): void
  /** Call from a tile's `dragstart` (after preventDefault) to drag the file into another app. */
  startDrag(id: number, path: string): void
  /** Absolute path of a dropped/pasted File (Electron webUtils). */
  pathForFile(file: File): string
  onStatus(cb: (s: IndexStatus) => void): () => void
  /** Window was summoned by the hotkey or tray. Focus the search box. */
  onShown(cb: () => void): () => void
  /** New shots were indexed. Refresh if the user is not mid-interaction. */
  onIndexed(cb: () => void): () => void
  /** Main asks the renderer to open settings (tray menu). */
  onOpenSettings(cb: () => void): () => void
}
