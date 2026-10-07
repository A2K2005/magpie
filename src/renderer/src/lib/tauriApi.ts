// MagpieApi over Tauri IPC: commands for calls, events for pushes. Commands are implemented in src-tauri/src/lib.rs.
import { invoke } from '@tauri-apps/api/core'
import { listen } from '@tauri-apps/api/event'
import type { MagpieApi } from '../../../shared/types'

const on =
  <T>(event: string) =>
  (cb: (data: T) => void): (() => void) => {
    const off = listen<T>(event, (e) => cb(e.payload))
    return () => void off.then((f) => f())
  }

export function createTauriApi(): MagpieApi {
  return {
    platform: /Mac/.test(navigator.userAgent)
      ? 'darwin'
      : /Windows/.test(navigator.userAgent)
        ? 'win32'
        : 'linux',
    search: (req) =>
      // Pasted image bytes go as the raw request body; JSON would inflate them 4×.
      // The rest of the request rides in a header (URL-encoded JSON).
      req.imageData
        ? invoke('search_image', req.imageData, {
            headers: {
              'x-magpie-req': encodeURIComponent(JSON.stringify({ ...req, imageData: undefined }))
            }
          })
        : invoke('search', { req }),
    getShot: (id) => invoke('get_shot', { id }),
    status: () => invoke('status'),
    stats: () => invoke('stats'),
    getSettings: () => invoke('get_settings'),
    setSettings: (patch) => invoke('set_settings', { patch }),
    suggestFolders: () => invoke('suggest_folders'),
    pickFolder: () => invoke('pick_folder'),
    copyText: (text) => invoke('copy_text', { text }),
    copyImage: (id) => invoke('copy_image', { id }),
    open: (id) => invoke('open_shot', { id }),
    reveal: (id) => invoke('reveal', { id }),
    trash: (ids) => invoke('trash_shots', { ids }),
    undoTrash: () => invoke('undo_trash'),
    pin: (id, pinned) => invoke('pin', { id, pinned }),
    reindex: () => invoke('reindex'),
    pause: (paused) => invoke('pause', { paused }),
    finishIndexing: (enabled) => invoke('finish_indexing', { enabled }),
    retryFailed: () => invoke('retry_failed'),
    repairModels: () => invoke('repair_models'),
    failures: () => invoke('failures'),
    updateMetadata: (id, metadata) => invoke('update_metadata', { id, metadata }),
    setRelevant: (id, query, relevant) => invoke('set_relevant', { id, query, relevant }),
    exportShots: (ids) => invoke('export_shots', { ids }),
    exportDiagnostics: () => invoke('export_diagnostics'),
    clearIndex: () => invoke('clear_index'),
    openExternal: (url) => invoke('open_external', { url }),
    hide: () => void invoke('hide_window'),
    startDrag: (id, path) => void invoke('start_drag', { id, path }),
    // Web views don't expose file paths; the UI falls back to the file's bytes.
    pathForFile: () => '',
    onStatus: on('magpie:status'),
    onShown: on('magpie:shown'),
    onIndexed: on('magpie:indexed'),
    onOpenSettings: on('magpie:openSettings')
  }
}
